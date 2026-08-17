# `container/sanitize.rs` Design Doc

*[日本語](sanitize.md)*

Design doc for `src/container/sanitize.rs`. Covers Phase 2 (sanitization) as defined by [architecture.md](../architecture.en.md). Provides only the detection/blocking logic for "Zip Bomb" and "Zip Slip (path traversal)" required by requirements spec section 2. [error.md](../error.en.md) already defines the error variants (`ZipBombDetected` / `ZipSlipDetected`) corresponding to this module's validation failures.

## Responsibility / Scope

- **Zip Slip protection**: validates that a ZIP entry name cannot escape the archive's logical root (`validate_entry_path`)
- **Zip Bomb protection**: provides a `Read` wrapper (`BoundedReader`) that enforces an uncompressed-size cap while streaming
- **Not responsible for**: extracting the ZIP archive itself or enumerating its entries (`container/mod.rs`), interpreting XML syntax or XXE protection (`parse/` — per the discussion recorded in architecture.md, the XXE requirement from requirements spec section 2 has already been settled as `parse/mod.rs`'s responsibility)

## Key Types (draft)

```rust
use crate::error::Error;
use std::io::{self, Read};

/// The default uncompressed-size cap for Phase 2 (in bytes). The concrete
/// value and whether the caller (via `lib.rs`'s public API) can override it
/// are undecided (see Open Question 1).
pub const DEFAULT_MAX_UNCOMPRESSED_SIZE: u64 = 512 * 1024 * 1024; // 512 MiB (provisional)

/// Validates that a ZIP entry name cannot escape the archive's logical root
/// (Zip Slip protection). `container/mod.rs` calls this for every entry name
/// as soon as it enumerates the central directory right after opening the
/// archive, and errors out immediately on the first invalid one — validation
/// happens once, up front, rather than lazily on individual entry access, so
/// an "untrusted entry name" can never reach any later stage of processing.
///
/// Checks performed (draft):
/// - Rejects the empty string
/// - Rejects absolute paths (starting with `/`)
/// - Rejects Windows drive-letter prefixes (e.g. `C:\...`)
/// - Rejects any path containing a `..` (parent directory) component
///
/// The implementation uses `std::path::Path::components()` purely for this
/// validation; the result is never interpreted or used as an actual
/// filesystem path (this library never extracts ZIP entries to disk, so the
/// traditional Zip Slip harm — an unintended file write — cannot occur
/// directly here; see Dependencies for why entry names are still validated).
pub fn validate_entry_path(name: &str) -> Result<(), Error>;

/// An internal marker type that `BoundedReader::read` embeds in an
/// `io::Error` once the cap is exceeded. The layer that ultimately converts
/// this into `Error::ZipBombDetected` (`container/mod.rs`, or wherever
/// `parse/` observes the I/O error) is expected to downcast via
/// `io::Error::into_inner()` to recover `limit` / `actual` (see Open
/// Question 3).
#[derive(Debug)]
pub(crate) struct LimitExceeded {
    pub limit: u64,
    pub actual: u64,
}

impl std::fmt::Display for LimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "uncompressed size {} bytes exceeds limit {} bytes", self.actual, self.limit)
    }
}
impl std::error::Error for LimitExceeded {}

/// A `Read` wrapper that enforces an uncompressed-size cap (Zip Bomb
/// protection). The ZIP header's self-declared uncompressed size can be
/// forged, so it is never trusted; instead, the number of bytes actually
/// read is counted while streaming, and an error is returned the moment the
/// cap is exceeded. `container/mod.rs` wraps each entry's decompression
/// stream with this before handing it to `parse/`.
pub struct BoundedReader<R> {
    inner: R,
    limit: u64,
    read_so_far: u64,
}

impl<R: Read> BoundedReader<R> {
    pub fn new(inner: R, limit: u64) -> Self {
        Self { inner, limit, read_so_far: 0 }
    }
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read_so_far += n as u64;
        if self.read_so_far > self.limit {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                LimitExceeded { limit: self.limit, actual: self.read_so_far },
            ));
        }
        Ok(n)
    }
}
```

## Dependencies

- Depends on: only [`error.rs`](../error.en.md) (to return `Error::ZipSlipDetected` / `Error::ZipBombDetected`). No dependency on any other module, including `model/`.
- Depended on by: `container/mod.rs` (applies `validate_entry_path` to every entry name when the archive is opened, and wraps each entry's decompression stream with `BoundedReader`)

The reason Zip Slip is validated even though this library never extracts entries to real disk is that entry names may still be trusted downstream. Specifically, `parse/relationships.rs` (Phase 1) resolves actual target files by combining relative paths inside `.rels` files (e.g. `../media/image1.png`) with entry names; a maliciously crafted entry name combined with such a relative path could otherwise resolve to an unintended entry. By validating every entry name as an allowlist-style check right when the archive is opened, this module structurally rules out the possibility of any later module ever having to deal with an "untrusted entry name."

## Error Handling Policy

- Like the parsing code, `validate_entry_path` never `panic`s; it returns `Result<(), Error>`. Any entry name that is ambiguous or cannot be interpreted is rejected (fail closed).
- Because `std::io::Read` constrains the return type, `BoundedReader::read` cannot directly return `crate::error::Error`; it returns an `io::Error` (with `LimitExceeded` embedded inside). The design of the boundary that converts this into `crate::error::Error::ZipBombDetected` on the caller side is not yet finalized (see Open Question 3).

## Testing Strategy

- `validate_entry_path` rejection cases: `"../../../etc/passwd"`, `"/etc/passwd"`, `"xl/../../evil"`, `"C:\\Windows\\System32\\evil"`, the empty string
- `validate_entry_path` acceptance cases: legitimate OPC entry names such as `"xl/worksheets/sheet1.xml"`, `"[Content_Types].xml"`, `"xl/_rels/workbook.xml.rels"`, `"xl/media/image1.png"`
- `BoundedReader`: verify reads succeed up to exactly `limit` bytes (boundary test)
- `BoundedReader`: verify a read that exceeds the limit by even one byte returns `Err`, with `LimitExceeded`'s `actual`/`limit` correctly set
- `BoundedReader`: verify ordinary reads before the cap is reached correctly count and pass through bytes

## Open Questions

1. **Default size cap and configurability**: whether `DEFAULT_MAX_UNCOMPRESSED_SIZE`'s concrete value (provisionally 512 MiB) is appropriate, and whether callers should be able to override the cap via `lib.rs`'s public API (e.g. `parse_workbook`), is to be finalized alongside `lib.rs`'s design.
2. **Scope of the cap: per-entry vs. cumulative**: the current `BoundedReader` only enforces a cap per individual entry (file). A Zip Bomb can also be constructed from many moderately-sized entries whose cumulative total becomes enormous, rather than a single extreme-ratio entry. Whether `container/mod.rs` should separately track and cap the cumulative uncompressed size across the whole archive is undecided.
3. **The conversion layer from `LimitExceeded` to `Error::ZipBombDetected`**: `BoundedReader` is expected to be handed to the `quick-xml` `Reader` owned by `parse/`, so the `io::Error` from a cap violation will likely propagate already wrapped by `quick-xml`. In that case it risks being treated as `XmlParse::source` (already type-erased to `Box<dyn Error>` per error.md), losing `Error::ZipBombDetected`'s structured `limit`/`actual` information. Which layer (`parse/mod.rs`'s secure Reader factory, or `pipeline.rs`) should walk `io::Error::into_inner()`, downcast to `LimitExceeded`, and reconstruct `ZipBombDetected` instead of `XmlParse` is to be finalized when `parse/` is designed.
4. **Whether to add compression-ratio-based detection**: the current design judges only by absolute uncompressed size. Whether `container/mod.rs` should additionally perform early screening using the ratio between the ZIP central directory's declared compressed and uncompressed sizes (e.g. flagging a ratio above 100:1) before actually decompressing anything is undecided; if added, whether that logic belongs in this file or in `container/mod.rs` is also undecided.
5. **Allowlisting entry names**: the current `validate_entry_path` uses a denylist approach (e.g. "must not contain `..`"). Whether to instead adopt a stricter allowlist that only permits entries matching known OPC namespace prefixes (`xl/`, `docProps/`, `_rels/`, `[Content_Types].xml`, etc.) is undecided.
