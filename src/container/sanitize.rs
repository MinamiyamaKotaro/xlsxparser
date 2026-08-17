//! Phase 2 sanitization: Zip Slip path validation and Zip Bomb
//! uncompressed-size limiting.

use crate::error::Error;
use std::io::{self, Read};

/// The default uncompressed-size cap for Phase 2, per individual entry (in
/// bytes).
pub const DEFAULT_MAX_UNCOMPRESSED_SIZE: u64 = 512 * 1024 * 1024; // 512 MiB

/// The default cumulative uncompressed-size cap for Phase 2, across the
/// whole archive (in bytes). Defends against the variant of Zip Bomb built
/// from many moderately-sized entries whose cumulative total becomes
/// enormous.
pub const DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB

/// Public configuration type callers use to set the Zip Bomb size caps.
/// `lib.rs` re-exports this at the crate root and uses it as the argument
/// to `parse_workbook_with_limits`/`parse_workbook_reader_with_limits`.
/// `Default` reuses `DEFAULT_MAX_UNCOMPRESSED_SIZE` /
/// `DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE` as-is, so the default-cap public
/// functions (`parse_workbook`/`parse_workbook_reader`) need only pass
/// `SizeLimits::default()` rather than duplicating the values.
#[derive(Debug, Clone, Copy)]
pub struct SizeLimits {
    /// The per-entry (sheet XML, etc.) uncompressed-size cap, in bytes.
    /// Passed straight through to `ZipContainer::with_max_entry_size`.
    pub max_entry_size: u64,
    /// The archive-wide cumulative uncompressed-size cap, in bytes. Passed
    /// straight through to `ZipContainer::with_max_total_size`.
    pub max_total_size: u64,
}

impl Default for SizeLimits {
    fn default() -> Self {
        Self {
            max_entry_size: DEFAULT_MAX_UNCOMPRESSED_SIZE,
            max_total_size: DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE,
        }
    }
}

/// Validates that a ZIP entry name cannot escape the archive's logical root
/// (Zip Slip protection). `container/mod.rs` calls this for every entry name
/// as soon as it enumerates the central directory right after opening the
/// archive, and errors out immediately on the first invalid one.
///
/// Checks performed:
/// - Rejects the empty string
/// - Rejects absolute paths (starting with `/`)
/// - Rejects any path containing a backslash (not a valid OPC/ZIP separator;
///   also covers Windows-style paths such as `C:\Windows\System32\evil`)
/// - Rejects Windows drive-letter prefixes (e.g. `C:evil`) independently of
///   the backslash check above
/// - Rejects any `/`-separated path containing a `..` (parent directory)
///   segment
///
/// Parsing is done with plain string operations rather than
/// `std::path::Path`, since `Path`'s component parsing is conditionally
/// compiled per target OS (e.g. backslash is only a separator, and drive
/// letters only recognized, on a `windows` target) — this validation must
/// behave identically regardless of which OS the library is built for.
pub fn validate_entry_path(name: &str) -> Result<(), Error> {
    let reject = || Error::ZipSlipDetected {
        entry_name: name.to_string(),
    };

    if name.is_empty() {
        return Err(reject());
    }
    if name.starts_with('/') {
        return Err(reject());
    }
    if name.contains('\\') {
        return Err(reject());
    }
    let bytes = name.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(reject());
    }
    if name.split('/').any(|segment| segment == "..") {
        return Err(reject());
    }

    Ok(())
}

/// An internal marker type that `BoundedReader::read` embeds in an
/// `io::Error` once a cap is exceeded. The layer that ultimately converts
/// this into `Error::ZipBombDetected` (`parse/mod.rs`'s conversion from a
/// quick-xml error into `crate::error::Error`) downcasts via
/// `io::Error::get_ref()` to recover `limit` / `actual`.
#[derive(Debug)]
pub(crate) struct LimitExceeded {
    pub limit: u64,
    pub actual: u64,
}

impl std::fmt::Display for LimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "uncompressed size {} bytes exceeds limit {} bytes",
            self.actual, self.limit
        )
    }
}

impl std::error::Error for LimitExceeded {}

/// A `Read` wrapper that enforces an uncompressed-size cap (Zip Bomb
/// protection). The ZIP header's self-declared uncompressed size can be
/// forged, so it is never trusted; instead, the number of bytes actually
/// read is counted while streaming, and an error is returned the moment a
/// cap is exceeded. `container/mod.rs` wraps each entry's decompression
/// stream with this before handing it to `parse/`.
///
/// In addition to the per-entry cap (`per_entry_limit`), it also adds every
/// read to `cumulative_read` — the running total across the whole archive —
/// and checks it against `cumulative_limit`. `cumulative_read` is a mutable
/// reference into a field owned by `ZipContainer`; no interior mutability
/// such as `Cell` is used.
#[derive(Debug)]
pub struct BoundedReader<'a, R> {
    inner: R,
    per_entry_limit: u64,
    per_entry_read: u64,
    cumulative_read: &'a mut u64,
    cumulative_limit: u64,
}

impl<'a, R: Read> BoundedReader<'a, R> {
    pub fn new(
        inner: R,
        per_entry_limit: u64,
        cumulative_read: &'a mut u64,
        cumulative_limit: u64,
    ) -> Self {
        Self {
            inner,
            per_entry_limit,
            per_entry_read: 0,
            cumulative_read,
            cumulative_limit,
        }
    }
}

impl<R: Read> Read for BoundedReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.per_entry_read += n as u64;
        *self.cumulative_read += n as u64;
        if self.per_entry_read > self.per_entry_limit {
            return Err(io::Error::other(LimitExceeded {
                limit: self.per_entry_limit,
                actual: self.per_entry_read,
            }));
        }
        if *self.cumulative_read > self.cumulative_limit {
            return Err(io::Error::other(LimitExceeded {
                limit: self.cumulative_limit,
                actual: *self.cumulative_read,
            }));
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_limits_default_matches_the_default_constants() {
        let limits = SizeLimits::default();
        assert_eq!(limits.max_entry_size, DEFAULT_MAX_UNCOMPRESSED_SIZE);
        assert_eq!(limits.max_total_size, DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE);
    }

    #[test]
    fn validate_entry_path_rejects_traversal_and_malformed_names() {
        let invalid = [
            "../../../etc/passwd",
            "/etc/passwd",
            "xl/../../evil",
            "C:\\Windows\\System32\\evil",
            "",
        ];
        for name in invalid {
            assert!(
                validate_entry_path(name).is_err(),
                "expected {name:?} to be rejected"
            );
        }
    }

    #[test]
    fn validate_entry_path_accepts_legitimate_opc_names() {
        let valid = [
            "xl/worksheets/sheet1.xml",
            "[Content_Types].xml",
            "xl/_rels/workbook.xml.rels",
            "xl/media/image1.png",
        ];
        for name in valid {
            assert!(
                validate_entry_path(name).is_ok(),
                "expected {name:?} to be accepted"
            );
        }
    }

    #[test]
    fn bounded_reader_allows_reads_up_to_per_entry_limit() {
        let data = [0u8; 10];
        let mut cumulative = 0u64;
        let mut reader = BoundedReader::new(&data[..], 10, &mut cumulative, 1000);

        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), 10);
        assert_eq!(cumulative, 10);
    }

    #[test]
    fn bounded_reader_rejects_read_exceeding_per_entry_limit() {
        let data = [0u8; 11];
        let mut cumulative = 0u64;
        let mut reader = BoundedReader::new(&data[..], 10, &mut cumulative, 1000);

        let mut out = Vec::new();
        let err = reader.read_to_end(&mut out).unwrap_err();
        let limit_exceeded = err
            .get_ref()
            .unwrap()
            .downcast_ref::<LimitExceeded>()
            .unwrap();
        assert_eq!(limit_exceeded.limit, 10);
        assert_eq!(limit_exceeded.actual, 11);
    }

    #[test]
    fn bounded_reader_enforces_cumulative_limit_across_calls() {
        let mut cumulative = 15u64; // already read from a prior entry
        let data = [0u8; 10];
        let mut reader = BoundedReader::new(&data[..], 1000, &mut cumulative, 20);

        let mut out = Vec::new();
        let err = reader.read_to_end(&mut out).unwrap_err();
        let limit_exceeded = err
            .get_ref()
            .unwrap()
            .downcast_ref::<LimitExceeded>()
            .unwrap();
        assert_eq!(limit_exceeded.limit, 20);
        assert_eq!(limit_exceeded.actual, 25);
    }

    #[test]
    fn bounded_reader_within_limits_passes_bytes_through_and_counts_correctly() {
        let data = b"hello world".to_vec();
        let mut cumulative = 5u64;
        let mut reader = BoundedReader::new(&data[..], 100, &mut cumulative, 100);

        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
        assert_eq!(cumulative, 5 + data.len() as u64);
    }
}
