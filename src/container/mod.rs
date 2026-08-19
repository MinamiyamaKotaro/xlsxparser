// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! ZIP (OPC) extraction entry point: safe file retrieval guarded against
//! Zip Slip / Zip Bomb (see [`sanitize`]).

pub mod sanitize;

use crate::error::Error;
use sanitize::{BoundedReader, DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE, DEFAULT_MAX_UNCOMPRESSED_SIZE};
use std::cell::OnceCell;
use std::collections::HashSet;
use std::io::{Read, Seek};

/// The entry point for ZIP extraction of a .xlsx (OPC) package. All entry
/// names in the central directory are validated via
/// `sanitize::validate_entry_path` at open time, so the type itself
/// guarantees that any entry name that makes it past `get_entry` is safe.
///
/// Constructed only via `open_reader` — `lib.rs::parse_workbook` opens its
/// own `std::fs::File` and passes it in as a generic `Read + Seek`, rather
/// than going through a path-based constructor on this type, since
/// `pipeline::run` itself is generic over the input and needs to support
/// non-file readers (`parse_workbook_reader`) the same way.
#[derive(Debug)]
pub struct ZipContainer<R> {
    archive: zip::ZipArchive<R>,
    max_entry_size: u64,
    /// Cap on the cumulative uncompressed size across the whole archive
    /// (Zip Bomb protection).
    max_total_size: u64,
    /// Running total of bytes decompressed so far via `get_entry`.
    /// `get_entry` lends this out to `BoundedReader` as `&mut`.
    total_read: u64,
    /// Every entry name in the archive, for `has_entry` to answer an
    /// existence-only query in O(1) without going through `get_entry`'s
    /// local-file-header read and `BoundedReader` construction (Issue #65
    /// PR review: image anchor resolution only ever needs to know whether a
    /// target exists, never its bytes). Built lazily on the first
    /// `has_entry` call rather than eagerly in `open_reader`: benchmarking
    /// showed eager construction added a measurable (~3-5%) `open_reader`
    /// regression on an entry-heavy archive (`thousand_sheets.xlsx`, 1008
    /// entries) — a cost every caller would pay even though most workbooks
    /// have no images and never call `has_entry` at all.
    entry_name_set: OnceCell<HashSet<String>>,
}

impl<R: Read + Seek> ZipContainer<R> {
    /// Opens an archive from any `Read + Seek` (e.g. an in-memory buffer).
    /// The ZIP format's central directory sits at the end of the file, so a
    /// seekable input is required.
    ///
    /// Once the central directory has been read successfully, every entry
    /// name is validated via `sanitize::validate_entry_path`. If even one is
    /// invalid, the whole archive is rejected with `Error::ZipSlipDetected`.
    pub fn open_reader(reader: R) -> Result<Self, Error> {
        let archive =
            zip::ZipArchive::new(reader).map_err(|e| Error::InvalidPackage(e.to_string()))?;
        for name in archive.file_names() {
            sanitize::validate_entry_path(name)?;
        }
        Ok(Self {
            archive,
            max_entry_size: DEFAULT_MAX_UNCOMPRESSED_SIZE,
            max_total_size: DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE,
            total_read: 0,
            entry_name_set: OnceCell::new(),
        })
    }

    /// Retrieves the decompressed stream for the named entry.
    ///
    /// - `name` is re-validated via `sanitize::validate_entry_path` on every
    ///   call (the open-time validation only covers entry names the archive
    ///   itself holds; `name` here may instead be a value that
    ///   `parse/relationships.rs` computed dynamically by combining a
    ///   relative path from a `.rels` file with an entry name, so it is
    ///   treated as an independent, untrusted input).
    /// - Returns `Ok(None)` if no matching entry exists in the archive. Only
    ///   the caller's context can tell whether that means a required part is
    ///   missing or a relationship target is dangling, so this method does
    ///   not construct an error itself.
    /// - The returned stream is wrapped in `BoundedReader`, so both the
    ///   per-entry cap (`max_entry_size`) and the archive-wide cumulative
    ///   cap (`max_total_size`) are already applied.
    ///
    /// Lookup is case-sensitive (`zip::ZipArchive::by_name`'s behavior),
    /// whereas OPC part names (ECMA-376 Part 2) are formally
    /// case-insensitive (ASCII case folding). In practice every real-world
    /// producer (Excel, Google Sheets, LibreOffice, Apache POI) keeps entry
    /// names and `.rels` `Target` references byte-identical, so this has not
    /// caused an observed interop failure; staying case-sensitive keeps
    /// `entry_names()`/`get_entry` simple and avoids allocating a
    /// lowercased-name lookup table for every archive. If a non-conforming
    /// producer is found in the wild, revisit by building a
    /// `HashMap<String, String>` (lowercased name -> original name) once at
    /// `open_reader` time, alongside the existing `validate_entry_path` pass
    /// (PR #21 review).
    pub fn get_entry(
        &mut self,
        name: &str,
    ) -> Result<Option<BoundedReader<'_, impl Read + '_>>, Error> {
        sanitize::validate_entry_path(name)?;

        let Self {
            archive,
            total_read,
            max_entry_size,
            max_total_size,
            ..
        } = self;

        match archive.by_name(name) {
            Ok(file) => Ok(Some(BoundedReader::new(
                file,
                *max_entry_size,
                total_read,
                *max_total_size,
            ))),
            Err(zip::result::ZipError::FileNotFound) => Ok(None),
            Err(e) => Err(Error::InvalidPackage(e.to_string())),
        }
    }

    /// Checks whether `name` exists in the archive, without reading it —
    /// re-validates `name` the same way `get_entry` does (see its doc: this
    /// may be a dynamically computed, untrusted path), then does an O(1)
    /// `entry_name_set` lookup instead of `get_entry`'s local-file-header
    /// read and `BoundedReader` construction. For a caller that only needs
    /// to know an entry exists — never its content — this is both a
    /// clearer statement of intent and cheaper than `get_entry(name)?
    /// .is_some()` followed by immediately dropping the reader.
    pub fn has_entry(&self, name: &str) -> Result<bool, Error> {
        sanitize::validate_entry_path(name)?;
        let set = self
            .entry_name_set
            .get_or_init(|| self.archive.file_names().map(str::to_string).collect());
        Ok(set.contains(name))
    }

    /// Lists all entry names in the archive (already validated at open
    /// time).
    ///
    /// Not called by `pipeline::run` today (it only ever fetches entries by
    /// a path it already knows), so this is currently exercised only by
    /// this module's own tests, which use it to confirm `open_reader`
    /// picked up the expected set of entries. Kept as the natural
    /// introspection counterpart to `get_entry`.
    #[allow(dead_code)]
    pub fn entry_names(&self) -> impl Iterator<Item = &str> {
        self.archive.file_names()
    }
}

impl<R> ZipContainer<R> {
    /// Opens with an explicitly set per-entry uncompressed-size cap for Zip
    /// Bomb protection. `pub(crate)` rather than exposed on the public API
    /// directly: `pipeline::run` calls this with the `SizeLimits` it
    /// receives from `lib.rs`'s public API
    /// (`parse_workbook_with_limits`/`parse_workbook_reader_with_limits`).
    pub(crate) fn with_max_entry_size(mut self, limit: u64) -> Self {
        self.max_entry_size = limit;
        self
    }

    /// Opens with an explicitly set archive-wide cumulative uncompressed-size
    /// cap for Zip Bomb protection. Same rationale as `with_max_entry_size`.
    pub(crate) fn with_max_total_size(mut self, limit: u64) -> Self {
        self.max_total_size = limit;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buf));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, data) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buf
    }

    #[test]
    fn open_reader_succeeds_for_valid_xlsx_shaped_zip() {
        let bytes = build_zip(&[
            ("[Content_Types].xml", b"<Types/>"),
            ("xl/workbook.xml", b"<workbook/>"),
        ]);
        let container = ZipContainer::open_reader(Cursor::new(bytes)).unwrap();
        let mut names: Vec<&str> = container.entry_names().collect();
        names.sort_unstable();
        assert_eq!(names, vec!["[Content_Types].xml", "xl/workbook.xml"]);
    }

    #[test]
    fn open_reader_rejects_corrupt_zip_bytes() {
        let err = ZipContainer::open_reader(Cursor::new(b"not a zip file".to_vec())).unwrap_err();
        assert!(matches!(err, Error::InvalidPackage(_)));
    }

    #[test]
    fn open_reader_rejects_archive_with_invalid_entry_name() {
        let bytes = build_zip(&[("../evil", b"payload")]);
        let err = ZipContainer::open_reader(Cursor::new(bytes)).unwrap_err();
        assert!(matches!(err, Error::ZipSlipDetected { .. }));
    }

    #[test]
    fn get_entry_returns_content_for_existing_name() {
        let bytes = build_zip(&[("xl/workbook.xml", b"<workbook/>")]);
        let mut container = ZipContainer::open_reader(Cursor::new(bytes)).unwrap();

        let mut out = Vec::new();
        container
            .get_entry("xl/workbook.xml")
            .unwrap()
            .unwrap()
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out, b"<workbook/>");
    }

    #[test]
    fn get_entry_returns_none_for_missing_name() {
        let bytes = build_zip(&[("xl/workbook.xml", b"<workbook/>")]);
        let mut container = ZipContainer::open_reader(Cursor::new(bytes)).unwrap();

        assert!(container.get_entry("xl/missing.xml").unwrap().is_none());
    }

    #[test]
    fn has_entry_true_for_existing_name() {
        let bytes = build_zip(&[("xl/workbook.xml", b"<workbook/>")]);
        let container = ZipContainer::open_reader(Cursor::new(bytes)).unwrap();

        assert!(container.has_entry("xl/workbook.xml").unwrap());
    }

    #[test]
    fn has_entry_false_for_missing_name() {
        let bytes = build_zip(&[("xl/workbook.xml", b"<workbook/>")]);
        let container = ZipContainer::open_reader(Cursor::new(bytes)).unwrap();

        assert!(!container.has_entry("xl/missing.xml").unwrap());
    }

    #[test]
    fn has_entry_rejects_malformed_name_even_if_absent() {
        let bytes = build_zip(&[("xl/workbook.xml", b"<workbook/>")]);
        let container = ZipContainer::open_reader(Cursor::new(bytes)).unwrap();

        let result = container.has_entry("../etc/passwd");
        assert!(matches!(result, Err(Error::ZipSlipDetected { .. })));
    }

    #[test]
    fn get_entry_rejects_malformed_name_even_if_absent() {
        let bytes = build_zip(&[("xl/workbook.xml", b"<workbook/>")]);
        let mut container = ZipContainer::open_reader(Cursor::new(bytes)).unwrap();

        // `BoundedReader<'_, impl Read>`'s opaque inner type doesn't implement
        // `Debug`, so `unwrap_err()` (which requires the `Ok` type to be
        // `Debug`) can't be used here; match instead.
        let result = container.get_entry("../etc/passwd");
        assert!(matches!(result, Err(Error::ZipSlipDetected { .. })));
    }

    #[test]
    fn get_entry_enforces_per_entry_size_limit() {
        let bytes = build_zip(&[("xl/workbook.xml", &[0u8; 100])]);
        let mut container = ZipContainer::open_reader(Cursor::new(bytes)).unwrap();
        container.max_entry_size = 10;

        let mut reader = container.get_entry("xl/workbook.xml").unwrap().unwrap();
        let mut out = Vec::new();
        assert!(reader.read_to_end(&mut out).is_err());
    }

    #[test]
    fn total_read_accumulates_and_enforces_cumulative_limit() {
        let bytes = build_zip(&[("xl/a.xml", &[0u8; 10]), ("xl/b.xml", &[0u8; 10])]);
        let mut container = ZipContainer::open_reader(Cursor::new(bytes)).unwrap();
        container.max_total_size = 15;

        let mut out = Vec::new();
        container
            .get_entry("xl/a.xml")
            .unwrap()
            .unwrap()
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(container.total_read, 10);

        let mut reader = container.get_entry("xl/b.xml").unwrap().unwrap();
        let mut out2 = Vec::new();
        assert!(reader.read_to_end(&mut out2).is_err());
    }
}
