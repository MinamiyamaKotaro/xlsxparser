// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! `xlsxparser` — a lightweight, high-performance `.xlsx` (OOXML) parser
//! library, purpose-built for the kind of files common in Japanese business
//! systems: sheets with an extreme number of rows/columns ("grid-paper
//! Excel") and heavy use of merged cells.
//!
//! ```no_run
//! let workbook = xlsxparser::parse_workbook("book.xlsx")?;
//! let json = xlsxparser::to_json_string(&workbook)?;
//! # Ok::<(), xlsxparser::Error>(())
//! ```
//!
//! # Security: CSV / formula injection
//!
//! Cell string values (including formula-computed result strings, `t="str"`)
//! pass through into [`CellValue::Text`] and the JSON output unchanged, with
//! no sanitization at any stage — this is safe as JSON output (`serde_json`
//! escapes correctly) but not necessarily as CSV or another spreadsheet
//! format. Callers who re-export parsed values into CSV or `.xlsx` are
//! responsible for their own formula-injection mitigations (e.g. escaping a
//! value that starts with `=`, `+`, `-`, or `@`), since a `.xlsx` input is
//! untrusted and this library performs no rewriting of cell content.

mod container;
mod error;
mod json;
mod model;
mod parse;
mod pipeline;
mod resolve;

pub use container::sanitize::SizeLimits;
pub use error::{Error, Result};
pub use json::{to_json_string, to_json_writer};
pub use model::{
    Alignment, AnchorMarker, Cell, CellRef, CellValue, ColWidthRange, DateTimeValue, Font, Image,
    ImageAnchor, ImageExtent, MergedRegion, ResolvedStyle, Sheet, SheetVisibility, StyleId,
    Workbook,
};

use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

/// Parses `.xlsx` from a file path — the most common public entry point.
/// Uses the default Zip Bomb size cap (`SizeLimits::default()`). To specify
/// the cap explicitly, use [`parse_workbook_with_limits`].
pub fn parse_workbook(path: impl AsRef<Path>) -> Result<Workbook> {
    parse_workbook_with_limits(path, SizeLimits::default())
}

/// [`parse_workbook`], plus letting the caller specify the Zip Bomb size cap
/// explicitly. `parse_workbook` is a thin wrapper that simply delegates
/// here with `SizeLimits::default()`; the actual logic — opening a
/// `std::fs::File` and delegating to the internal pipeline — lives only in
/// this function. Beyond a failure of `File::open` itself, any I/O error
/// arising during ZIP extraction or XML streaming with `path` left unset
/// (`None`) is backfilled with the file path this function already knows
/// before being returned.
pub fn parse_workbook_with_limits(path: impl AsRef<Path>, limits: SizeLimits) -> Result<Workbook> {
    let path = path.as_ref();
    let file = File::open(path).map_err(|source| Error::Io {
        path: Some(path.to_path_buf()),
        source,
    })?;
    pipeline::run(file, limits).map_err(|err| fill_io_path(err, path))
}

/// Backfills the file path `parse_workbook_with_limits` already knows into
/// an `Error::Io { path: None, .. }` propagated from the pipeline. Any other
/// variant is returned unchanged. `Error::XmlParse` /
/// `Error::MissingRequiredElement` also carry a `path` field, but theirs
/// names a part within the OPC package (e.g. `"xl/worksheets/sheet1.xml"`)
/// — a different meaning from a filesystem path — so they are excluded from
/// backfilling.
fn fill_io_path(err: Error, path: &Path) -> Error {
    match err {
        Error::Io { path: None, source } => Error::Io {
            path: Some(path.to_path_buf()),
            source,
        },
        other => other,
    }
}

/// Parses `.xlsx` from any `Read + Seek` input (an in-memory buffer, a
/// fully-read HTTP response body, etc.) — a general-purpose entry point for
/// callers that don't go through the filesystem. Requiring a seekable input
/// to read the ZIP central directory simply carries forward
/// `ZipContainer::open_reader`'s constraint (a purely streaming `Read`-only
/// input cannot be opened this way). Uses the default Zip Bomb size cap
/// (`SizeLimits::default()`). To specify the cap explicitly, use
/// [`parse_workbook_reader_with_limits`].
pub fn parse_workbook_reader<R: Read + Seek>(reader: R) -> Result<Workbook> {
    parse_workbook_reader_with_limits(reader, SizeLimits::default())
}

/// [`parse_workbook_reader`], plus letting the caller specify the Zip Bomb
/// size cap explicitly. `parse_workbook_reader` is a thin wrapper that
/// simply delegates here with `SizeLimits::default()`.
pub fn parse_workbook_reader_with_limits<R: Read + Seek>(
    reader: R,
    limits: SizeLimits,
) -> Result<Workbook> {
    pipeline::run(reader, limits)
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

    const RELS_XML: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#;

    const WORKBOOK_XML: &[u8] = br#"<?xml version="1.0"?>
<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
    <sheet name="Sheet1" sheetId="1" r:id="rId1"/>
  </sheets>
</workbook>"#;

    const STYLES_XML: &[u8] = br#"<styleSheet><cellXfs><xf numFmtId="0"/></cellXfs></styleSheet>"#;

    const WORKSHEET_XML: &[u8] =
        br#"<worksheet><sheetData><row r="1"><c r="A1"><v>42</v></c></row></sheetData></worksheet>"#;

    fn minimal_xlsx() -> Vec<u8> {
        build_zip(&[
            ("xl/_rels/workbook.xml.rels", RELS_XML),
            ("xl/workbook.xml", WORKBOOK_XML),
            ("xl/styles.xml", STYLES_XML),
            ("xl/worksheets/sheet1.xml", WORKSHEET_XML),
        ])
    }

    #[test]
    fn parse_workbook_reads_a_valid_file() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "xlsxparser-test-{}-{}.xlsx",
            std::process::id(),
            "parse_workbook_reads_a_valid_file"
        ));
        std::fs::write(&path, minimal_xlsx()).unwrap();

        let result = parse_workbook(&path);
        std::fs::remove_file(&path).ok();

        let workbook = result.unwrap();
        assert_eq!(workbook.sheets().len(), 1);
    }

    #[test]
    fn parse_workbook_missing_file_returns_io_error_with_path() {
        let path = std::env::temp_dir().join("xlsxparser-test-does-not-exist.xlsx");
        let err = parse_workbook(&path).unwrap_err();
        match err {
            Error::Io { path: Some(p), .. } => assert_eq!(p, path),
            other => panic!("expected Error::Io {{ path: Some(..), .. }}, got {other:?}"),
        }
    }

    #[test]
    fn fill_io_path_rewrites_none_path_only() {
        let path = Path::new("book.xlsx");

        let with_none = Error::Io {
            path: None,
            source: std::io::Error::other("boom"),
        };
        match fill_io_path(with_none, path) {
            Error::Io { path: Some(p), .. } => assert_eq!(p, path),
            other => panic!("expected Error::Io {{ path: Some(..), .. }}, got {other:?}"),
        }

        let with_some = Error::Io {
            path: Some(std::path::PathBuf::from("already-set.xlsx")),
            source: std::io::Error::other("boom"),
        };
        match fill_io_path(with_some, path) {
            Error::Io { path: Some(p), .. } => {
                assert_eq!(p, std::path::PathBuf::from("already-set.xlsx"))
            }
            other => panic!("expected Error::Io {{ path: Some(..), .. }}, got {other:?}"),
        }

        let other_variant = Error::XmlParse {
            path: "xl/worksheets/sheet1.xml".to_string(),
            source: Box::new(std::io::Error::other("boom")),
        };
        assert!(matches!(
            fill_io_path(other_variant, path),
            Error::XmlParse { .. }
        ));
    }

    #[test]
    fn parse_workbook_reader_reads_valid_bytes() {
        let workbook = parse_workbook_reader(Cursor::new(minimal_xlsx())).unwrap();
        assert_eq!(workbook.sheets().len(), 1);
    }

    #[test]
    fn parse_workbook_and_parse_workbook_reader_agree() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "xlsxparser-test-{}-{}.xlsx",
            std::process::id(),
            "parse_workbook_and_parse_workbook_reader_agree"
        ));
        let bytes = minimal_xlsx();
        std::fs::write(&path, &bytes).unwrap();

        let from_path = parse_workbook(&path);
        std::fs::remove_file(&path).ok();
        let from_path = from_path.unwrap();
        let from_reader = parse_workbook_reader(Cursor::new(bytes)).unwrap();

        assert_eq!(from_path.sheets().len(), from_reader.sheets().len());
        assert_eq!(from_path.sheets()[0].name, from_reader.sheets()[0].name);
        assert_eq!(
            from_path.sheets()[0].get(CellRef { row: 1, col: 1 }),
            from_reader.sheets()[0].get(CellRef { row: 1, col: 1 })
        );
    }

    #[test]
    fn with_limits_variants_match_the_default_cap_functions() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "xlsxparser-test-{}-{}.xlsx",
            std::process::id(),
            "with_limits_variants_match_the_default_cap_functions"
        ));
        let bytes = minimal_xlsx();
        std::fs::write(&path, &bytes).unwrap();

        let default_from_path = parse_workbook(&path).unwrap();
        let explicit_from_path = parse_workbook_with_limits(&path, SizeLimits::default()).unwrap();
        std::fs::remove_file(&path).ok();

        let default_from_reader = parse_workbook_reader(Cursor::new(bytes.clone())).unwrap();
        let explicit_from_reader =
            parse_workbook_reader_with_limits(Cursor::new(bytes), SizeLimits::default()).unwrap();

        assert_eq!(
            default_from_path.sheets()[0].name,
            explicit_from_path.sheets()[0].name
        );
        assert_eq!(
            default_from_reader.sheets()[0].name,
            explicit_from_reader.sheets()[0].name
        );
    }

    #[test]
    fn caller_supplied_size_limits_are_honored_by_the_public_api() {
        // Succeeds under the default cap...
        parse_workbook_reader(Cursor::new(minimal_xlsx())).unwrap();

        // ...but a caller-supplied max_entry_size too small to hold even
        // xl/workbook.xml turns the same input into Error::ZipBombDetected,
        // proving the public `_with_limits` functions actually forward
        // `limits` through to the pipeline rather than ignoring it.
        let tiny_limits = SizeLimits {
            max_entry_size: 1,
            max_total_size: SizeLimits::default().max_total_size,
        };
        let err = parse_workbook_reader_with_limits(Cursor::new(minimal_xlsx()), tiny_limits)
            .unwrap_err();
        assert!(matches!(err, Error::ZipBombDetected { .. }));
    }

    #[test]
    fn parse_workbook_output_chains_into_to_json_string() {
        let workbook = parse_workbook_reader(Cursor::new(minimal_xlsx())).unwrap();
        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["sheets"][0]["name"], "Sheet1");
    }

    #[test]
    fn corrupt_xlsx_errors_propagate_unchanged() {
        let err = parse_workbook_reader(Cursor::new(b"not a zip file".to_vec())).unwrap_err();
        assert!(matches!(err, Error::InvalidPackage(_)));

        let missing_rels = build_zip(&[("xl/workbook.xml", WORKBOOK_XML)]);
        let err = parse_workbook_reader(Cursor::new(missing_rels)).unwrap_err();
        assert!(matches!(err, Error::MissingRelationshipPart(_)));
    }

    #[test]
    fn public_types_are_reachable_from_the_crate_root() {
        // A compile-time check: if any of these names weren't re-exported at
        // the crate root, this module simply wouldn't compile.
        fn assert_reachable<T>() {}
        assert_reachable::<crate::Workbook>();
        assert_reachable::<crate::Sheet>();
        assert_reachable::<crate::Cell>();
        assert_reachable::<crate::CellValue>();
        assert_reachable::<crate::CellRef>();
        assert_reachable::<crate::SheetVisibility>();
        assert_reachable::<crate::MergedRegion>();
        assert_reachable::<crate::ResolvedStyle>();
        assert_reachable::<crate::StyleId>();
        assert_reachable::<crate::Alignment>();
        assert_reachable::<crate::DateTimeValue>();
        assert_reachable::<crate::SizeLimits>();
        assert_reachable::<crate::Error>();
        fn _assert_result_reachable(_: crate::Result<()>) {}
    }
}
