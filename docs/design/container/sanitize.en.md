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

/// The default uncompressed-size cap for Phase 2, per individual entry (in
/// bytes). The concrete value and whether the caller (via `lib.rs`'s public
/// API) can override it are undecided (see Open Question 1).
pub const DEFAULT_MAX_UNCOMPRESSED_SIZE: u64 = 512 * 1024 * 1024; // 512 MiB (provisional)

/// The default cumulative uncompressed-size cap for Phase 2, across the
/// whole archive (in bytes). Defends against the variant of Zip Bomb built
/// from many moderately-sized entries whose cumulative total becomes
/// enormous (see [container/mod.md](mod.en.md). Reflects feedback from the
/// PR #7 review).
pub const DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB (provisional)

/// Validates that a ZIP entry name cannot escape the archive's logical root
/// (Zip Slip protection). `container/mod.rs` calls this for every entry name
/// as soon as it enumerates the central directory right after opening the
/// archive, and errors out immediately on the first invalid one — validation
/// happens once, up front, rather than lazily on individual entry access, so
/// an "untrusted entry name" can never reach any later stage of processing.
///
/// Checks performed:
/// - Rejects the empty string
/// - Rejects absolute paths (starting with `/`)
/// - Rejects any path containing a backslash (not a valid OPC/ZIP
///   separator; also covers Windows-style paths such as
///   `C:\Windows\System32\evil`)
/// - Rejects Windows drive-letter prefixes (e.g. `C:evil`) independently of
///   the backslash check above
/// - Rejects any `/`-separated path containing a `..` (parent directory)
///   segment
///
/// Implemented with plain string operations (`starts_with`/`contains`/
/// `split('/')`) rather than `std::path::Path`, finalized at implementation
/// time (PR #7's draft had proposed `Path::components()`): `Path`'s
/// component parsing is conditionally compiled per target OS — e.g.
/// backslash is only treated as a separator, and drive letters only
/// recognized, when built for a `windows` target — so it would not reject
/// `C:\Windows\System32\evil` the same way on a non-Windows build. This
/// validation must behave identically regardless of which OS the library is
/// built for. The result is never interpreted or used as an actual
/// filesystem path (this library never extracts ZIP entries to disk, so the
/// traditional Zip Slip harm — an unintended file write — cannot occur
/// directly here; see Dependencies for why entry names are still validated).
pub fn validate_entry_path(name: &str) -> Result<(), Error>;

/// An internal marker type that `BoundedReader::read` embeds in an
/// `io::Error` once a cap is exceeded. The layer that ultimately converts
/// this into `Error::ZipBombDetected` (the boundary where `parse/` converts
/// a quick-xml error into `crate::error::Error`; see Error Handling Policy)
/// is expected to downcast via `io::Error::get_ref()` to recover `limit` /
/// `actual` (Open Question 3 is resolved per the PR #7 review; see Error
/// Handling Policy).
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
/// read is counted while streaming, and an error is returned the moment a
/// cap is exceeded. `container/mod.rs` wraps each entry's decompression
/// stream with this before handing it to `parse/`.
///
/// In addition to the per-entry cap (`per_entry_limit`), it also adds every
/// read to `cumulative_read` — the running total across the whole archive —
/// and checks it against `cumulative_limit` (see
/// [container/mod.md](mod.en.md). Reflects feedback from the PR #7 review;
/// resolves Open Question 2). `cumulative_read` is a mutable reference into
/// a field owned by `ZipContainer`; no interior mutability such as `Cell` is
/// used. Because `get_entry` already requires `&mut self` (guaranteeing
/// exclusive access at that point), it is enough to take disjoint field
/// borrows of the `archive` field (for the entry's read stream) and the
/// `cumulative_read` field from the same `self`.
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
        Self { inner, per_entry_limit, per_entry_read: 0, cumulative_read, cumulative_limit }
    }
}

impl<'a, R: Read> Read for BoundedReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.per_entry_read += n as u64;
        *self.cumulative_read += n as u64;
        if self.per_entry_read > self.per_entry_limit {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                LimitExceeded { limit: self.per_entry_limit, actual: self.per_entry_read },
            ));
        }
        if *self.cumulative_read > self.cumulative_limit {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                LimitExceeded { limit: self.cumulative_limit, actual: *self.cumulative_read },
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
- Because `std::io::Read` constrains the return type, `BoundedReader::read` cannot directly return `crate::error::Error`; it returns an `io::Error` (with `LimitExceeded` embedded inside).

  The boundary that ultimately converts this `io::Error` into `crate::error::Error::ZipBombDetected` is placed **not in `pipeline.rs`, but in the point where `parse/` converts a quick-xml error into `crate::error::Error`** (planned to live alongside `parse/mod.rs`'s secure Reader factory) (finalized following the PR #7 review; resolves the former Open Question 3).

  Rationale: per the design settled in [error.md](../error.en.md), `Error::XmlParse::source` is already type-erased to `Box<dyn std::error::Error + Send + Sync + 'static>`. `pipeline.rs` only ever receives this already-erased `Error::XmlParse`, and `pipeline.rs` does not depend on `quick-xml` (by design, to avoid a public dependency) — so it has no way to downcast to `quick_xml::Error`'s concrete variants (e.g. `Io(io::Error)`). `parse/`, on the other hand, already depends on `quick-xml`, so at the point where it converts a `quick_xml::Error` into `crate::error::Error` — while it still holds the `io::Error` before it gets type-erased — it can call `io::Error::get_ref()` → `.downcast_ref::<LimitExceeded>()`. If that succeeds, it returns `Error::ZipBombDetected { limit, actual }` instead of `Error::XmlParse`.

  Consolidating this conversion logic into a single function callable from every `parse/*.rs` file (e.g. `parse/mod.rs::convert_xml_error`) localizes the risk of a missed conversion (the exact function signature is to be finalized when `parse/` is designed).

## Testing Strategy

- `validate_entry_path` rejection cases: `"../../../etc/passwd"`, `"/etc/passwd"`, `"xl/../../evil"`, `"C:\\Windows\\System32\\evil"`, the empty string
- `validate_entry_path` acceptance cases: legitimate OPC entry names such as `"xl/worksheets/sheet1.xml"`, `"[Content_Types].xml"`, `"xl/_rels/workbook.xml.rels"`, `"xl/media/image1.png"`
- `BoundedReader`: verify reads succeed up to exactly the per-entry limit (`per_entry_limit` bytes) (boundary test)
- `BoundedReader`: verify a read that exceeds the per-entry limit by even one byte returns `Err`, with `LimitExceeded`'s `actual`/`limit` correctly reflecting `per_entry_limit`
- `BoundedReader`: verify that even when a single entry stays within its own limit, cumulative reads spanning multiple entries that exceed `cumulative_limit` return `Err`, with `LimitExceeded`'s `actual`/`limit` correctly reflecting `cumulative_limit` (including verifying the cumulative counter correctly carries over across calls)
- `BoundedReader`: verify ordinary reads before either cap is reached correctly count and pass through bytes, and that `cumulative_read` is incremented correctly

## Open Questions

1. **Default size caps and configurability**: whether the concrete values of `DEFAULT_MAX_UNCOMPRESSED_SIZE` (provisionally 512 MiB) and `DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE` (provisionally 2 GiB) are appropriate, and whether callers should be able to override them via `lib.rs`'s public API (e.g. `parse_workbook`), is to be finalized alongside `lib.rs`'s design.
2. ~~Scope of the cap: per-entry vs. cumulative~~ → **Resolved**: `BoundedReader` now monitors both a per-entry cap (`per_entry_limit`) and a cumulative cap across the whole archive (`cumulative_limit`) simultaneously. The cumulative counter itself is a field owned by `ZipContainer` in [container/mod.md](mod.en.md), passed into `BoundedReader` as `&mut u64` on each `get_entry` call (reflects feedback from the PR #7 review).
3. ~~The conversion layer from `LimitExceeded` to `Error::ZipBombDetected`~~ → **Resolved**: the downcast happens where `parse/` converts a `quick_xml::Error` into `crate::error::Error` — not in `pipeline.rs`. See Error Handling Policy for the rationale and details (reflects feedback from the PR #7 review; the alternative initially considered — giving `ZipContainer` a shared flag via `Cell` that `pipeline.rs` checks — was not adopted, since it would create an ongoing risk of a missed check outside `container/sanitize.rs`'s own visibility, and would require extra interior mutability compared to converting at the `parse/` layer).
4. **Whether to add compression-ratio-based detection**: the current design judges only by absolute uncompressed size. Whether `container/mod.rs` should additionally perform early screening using the ratio between the ZIP central directory's declared compressed and uncompressed sizes (e.g. flagging a ratio above 100:1) before actually decompressing anything is undecided; if added, whether that logic belongs in this file or in `container/mod.rs` is also undecided.
5. **Allowlisting entry names**: the current `validate_entry_path` uses a denylist approach (e.g. "must not contain `..`"). Whether to instead adopt a stricter allowlist that only permits entries matching known OPC namespace prefixes (`xl/`, `docProps/`, `_rels/`, `[Content_Types].xml`, etc.) is undecided.
