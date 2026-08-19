# `container/mod.rs` Design Doc

*[日本語](mod.md)*

Design doc for `src/container/mod.rs`. Implements the responsibility `architecture.md` assigns to `container/`: "the entry point for ZIP (OPC) extraction, safe file retrieval." `pipeline.rs` owns the `ZipContainer` defined here and controls resource-disposal timing across phases (architecture.md design policy 3).

## Responsibility / Scope

- Opens a ZIP (OPC) archive and reads all entry names from its central directory
- At open time, validates every entry name in one pass via [`container/sanitize.rs`](sanitize.en.md)'s `validate_entry_path`, immediately rejecting an archive that contains any invalid entry name (fail closed)
- Provides `get_entry`, the "safe file retrieval" gateway that only ever hands out a given entry's decompressed stream wrapped in the Zip-Bomb-protecting `BoundedReader` ([sanitize.md](sanitize.en.md))
- Provides `has_entry`, an existence-only query backed by a `HashSet<String>` — for a caller that only needs to know whether a target exists (never its bytes), this avoids `get_entry`'s local-file-header read and `BoundedReader` construction (Issue #65 PR review; `pipeline.rs`'s image-anchor resolution is the first such caller). The set is built lazily, on the first `has_entry` call, rather than eagerly in `open_reader`: benchmarking showed eager construction cost every workbook a measurable (~3-5%) `open_reader` regression on an entry-heavy archive, a price most callers would pay for a feature only image-anchor resolution uses
- **Not responsible for**: the Zip Bomb/Zip Slip detection logic itself (`container/sanitize.rs`), XML syntax interpretation or XXE protection (`parse/`), interpreting `_rels` content or mapping sheet IDs to file paths (`parse/relationships.rs`), deciding which specific parts (e.g. `[Content_Types].xml`, `xl/workbook.xml`) are required (the caller — this file only handles "can the named entry be retrieved safely," not which parts are mandatory)

## Key Types (draft)

```rust
use crate::container::sanitize::{
    self, BoundedReader, DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE, DEFAULT_MAX_UNCOMPRESSED_SIZE,
};
use crate::error::Error;
use std::io::{Read, Seek};
use std::path::Path;

/// The entry point for ZIP extraction of a .xlsx (OPC) package. All entry
/// names in the central directory are validated via
/// `sanitize::validate_entry_path` at open time, so the type itself
/// guarantees that any entry name that makes it past `get_entry` is safe.
///
/// Which ZIP-handling crate's type is held internally is undecided (see
/// Open Question 1).
pub struct ZipContainer<R> {
    archive: R, // placeholder standing in for whatever archive type the chosen ZIP crate provides
    max_entry_size: u64,
    /// Cap on the cumulative uncompressed size across the whole archive
    /// (Zip Bomb protection; see [sanitize.md](sanitize.en.md). Reflects
    /// feedback from the PR #7 review).
    max_total_size: u64,
    /// Running total of bytes decompressed so far via `get_entry`.
    /// `get_entry` lends this out to `BoundedReader` as `&mut` (see
    /// Dependencies).
    total_read: u64,
}

impl ZipContainer<std::fs::File> {
    /// Opens an archive from a file path.
    pub fn open(path: &Path) -> Result<Self, Error> {
        Self::open_reader(std::fs::File::open(path).map_err(|source| Error::Io {
            path: Some(path.to_path_buf()),
            source,
        })?)
    }
}

impl<R: Read + Seek> ZipContainer<R> {
    /// Opens an archive from any `Read + Seek` (e.g. an in-memory buffer).
    /// The ZIP format's central directory sits at the end of the file, so a
    /// seekable input is required (a purely streaming `Read`-only input
    /// cannot be opened this way).
    ///
    /// Once the central directory has been read successfully, every entry
    /// name is validated via `sanitize::validate_entry_path`. If even one is
    /// invalid, the whole archive is rejected with `Error::ZipSlipDetected`.
    pub fn open_reader(reader: R) -> Result<Self, Error> {
        let _ = reader;
        unimplemented!()
    }

    /// Retrieves the decompressed stream for the named entry.
    ///
    /// - `name` is re-validated via `sanitize::validate_entry_path` on every
    ///   call (the open-time validation only covers entry names the archive
    ///   itself holds; `name` here may instead be a value that
    ///   `parse/relationships.rs` computed dynamically by combining a
    ///   relative path from a `.rels` file with an entry name, so it is
    ///   treated as an independent, untrusted input — see Dependencies).
    /// - Returns `Ok(None)` if no matching entry exists in the archive.
    ///   Non-existence is not itself an error condition: only the caller's
    ///   context can tell whether it means a required part is missing
    ///   (`Error::InvalidPackage`) or a relationship target is dangling
    ///   (`Error::DanglingRelationship`), so this method does not construct
    ///   an error itself.
    /// - The returned stream is wrapped in `BoundedReader`, so both the
    ///   per-entry cap (`max_entry_size`) and the archive-wide cumulative
    ///   cap (`max_total_size`) are already applied.
    pub fn get_entry(&mut self, name: &str) -> Result<Option<BoundedReader<'_, impl Read + '_>>, Error> {
        sanitize::validate_entry_path(name)?;
        // The entry's read stream (from the `archive` field) and a mutable
        // reference to `total_read` (passed as `BoundedReader::new`'s
        // `cumulative_read` argument) can coexist by taking disjoint field
        // borrows of the same `self`. No interior mutability such as `Cell`
        // is needed (see sanitize.md).
        let Self { archive, total_read, max_entry_size, max_total_size, .. } = self;
        let _ = (archive, name, *max_entry_size, total_read, *max_total_size);
        unimplemented!()
    }

    /// Lists all entry names in the archive (already validated at open time).
    pub fn entry_names(&self) -> impl Iterator<Item = &str> {
        std::iter::empty()
    }
}

impl<R> ZipContainer<R> {
    /// Opens with an explicitly set per-entry uncompressed-size cap for Zip
    /// Bomb protection. When unset, `DEFAULT_MAX_UNCOMPRESSED_SIZE`
    /// ([sanitize.md](sanitize.en.md)) is assumed to apply (the concrete
    /// builder-API shape is undecided; see Open Question 3).
    fn with_max_entry_size(mut self, limit: u64) -> Self {
        self.max_entry_size = limit;
        self
    }

    /// Opens with an explicitly set archive-wide cumulative uncompressed-size
    /// cap for Zip Bomb protection. When unset,
    /// `DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE` ([sanitize.md](sanitize.en.md))
    /// is assumed to apply (resolves Open Question 4 following the PR #7
    /// review; the concrete builder-API shape is undecided the same way as
    /// `with_max_entry_size`, see Open Question 3).
    fn with_max_total_size(mut self, limit: u64) -> Self {
        self.max_total_size = limit;
        self
    }
}
```

## Dependencies

- Depends on: [`container/sanitize.rs`](sanitize.en.md) (`validate_entry_path`, `BoundedReader`, `DEFAULT_MAX_UNCOMPRESSED_SIZE`, `DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE`) and [`error.rs`](../error.en.md). No dependency on `model/` or `parse/`.
- Depended on by: `pipeline.rs` only. Per architecture.md design policy 3 ("`container` and `parse` go back and forth tightly, but this call ordering and resource lifecycle management is centralized in `pipeline.rs` so other modules don't need to know about each other directly"), no module under `parse/` knows about `container::ZipContainer` directly — `pipeline.rs` passes along the byte stream it obtained via `get_entry`.

Why `get_entry` re-validates `name` on every call: the open-time application of `validate_entry_path` only covers entry names the archive itself actually holds (static strings from the central directory). But the `name` passed to `get_entry` may instead be a string that `parse/relationships.rs` (Phase 1), via `pipeline.rs`, computed dynamically by combining a relative-path notation from a `.rels` file (e.g. `../media/image1.png`) with an entry name. If that computation has any normalization gap (e.g. an unresolved `..`), a path that slipped past the open-time check could reach `get_entry`. It is therefore treated as an independent, untrusted input and validated again every time (defense in depth).

The design where `get_entry` requires `&mut self` and ties the returned value's lifetime to that borrow (`impl Read + '_`) naturally matches the sequential access pattern architecture.md already describes — "`container` and `parse` go back and forth tightly: fetch bytes → parse → fetch the next entry based on the result." It encodes in the type system the assumption that there is no need to hold multiple entries open for processing at the same time.

## Error Handling Policy

- `open` / `open_reader` return `Error::InvalidPackage` when the ZIP archive itself is corrupt. Whether the underlying ZIP crate's error is simply stringified, or held in a dedicated type-erased `Box<dyn Error>` field the way `error.md`'s `XmlParse` does, is to be revisited once the crate is chosen (Open Question 1, tied to [error.md Open Question 1](../error.en.md)).
- `open_reader` rejects the entire archive with `Error::ZipSlipDetected` if any entry name in the central directory fails `validate_entry_path` — there is no partial-acceptance fallback that uses only the "safe" entries.
- `get_entry` represents a missing entry as `Ok(None)` rather than via `Result`'s error path (the same design principle `model::Sheet::get` uses to represent a blank cell as `None`; see [model/sheet.md](../model/sheet.en.md)). `has_entry` mirrors this as a plain `bool` inside `Ok` — its `Result` wrapping exists only for `validate_entry_path`'s own failure (an untrusted, dynamically computed `name` that fails Zip Slip validation), a distinct concern from "no matching entry."
- Constructing an error for a missing required part (`[Content_Types].xml`, `xl/workbook.xml`, etc.) is not this file's responsibility. `ZipContainer` stays a generic container layer that only handles "safe file retrieval," with no OPC-specific semantics about which parts are mandatory (resolves Open Question 5 following the PR #7 review). Whether an `Ok(None)` from `get_entry` should become `Error::InvalidPackage` or `Error::DanglingRelationship` is for `pipeline.rs` / `parse/relationships.rs` to decide.
- The conversion from an `io::Error` raised mid-read from the `BoundedReader` returned by `get_entry` into `Error::ZipBombDetected` happens at the boundary where `parse/` converts `quick_xml::Error` into `crate::error::Error` (see [sanitize.md Error Handling Policy](sanitize.en.md); Open Question 3 is resolved).

## Testing Strategy

- Verify that `open`-ing a minimal, valid `.xlsx`-shaped ZIP succeeds and `entry_names()` returns the expected set of entries
- Verify that `open_reader`-ing corrupted ZIP bytes returns `Error::InvalidPackage`
- Verify that `open_reader`-ing a ZIP whose central directory contains an invalid entry name (e.g. `"../evil"`) returns `Error::ZipSlipDetected` and rejects the whole archive (one bad entry rejects everything)
- Verify that calling `get_entry` with a name that exists returns `Ok(Some(..))` with the expected byte content
- Verify that calling `get_entry` with a name that does not exist returns `Ok(None)` (not an error)
- Verify that calling `get_entry` with a malformed `name` such as `"../etc/passwd"` returns `Error::ZipSlipDetected` regardless of whether such an entry actually exists in the archive (a defense-in-depth test simulating a path that slipped past open-time validation)
- Verify that `has_entry` returns `Ok(true)`/`Ok(false)` for an existing/missing name respectively, and `Err(Error::ZipSlipDetected)` for a malformed name even if absent (the same defense-in-depth property as `get_entry`)
- Verify that reading beyond `max_entry_size` from the stream `get_entry` returns produces an error (a wiring test against `sanitize::BoundedReader`; `BoundedReader`'s own logic is verified in [sanitize.md](sanitize.en.md))
- Verify that `total_read` correctly accumulates across multiple `get_entry` calls, and that exceeding `max_total_size` produces an error (a wiring test for the cumulative counter `ZipContainer` passes into `BoundedReader`)

## Open Questions

1. ~~Which external crate to use for ZIP handling~~ → **Resolved**: the `zip` crate (v8), matching [error.md Open Question 1](../error.en.md). `open`/`open_reader` still stringify `zip::result::ZipError` into `Error::InvalidPackage(String)` rather than holding it as a dedicated `#[source]`-carrying variant — kept as the simpler catch-all for now, revisit if callers need to match on specific ZIP failure kinds.
2. ~~Return type design for `get_entry`~~ → **Resolved**: adopt `impl Read + '_` (RPIT, tied to `self`'s borrow). This library's processing pipeline is a fully sequential access pattern — "read rels → read SST → read worksheets one after another" — with no design need to hold multiple entries' streams open at once. `impl Read + '_` has no allocation cost and lets the compiler statically rule out opening multiple streams at the same time (a borrow conflict), which is preferable to `Box<dyn Read + '_>` (reflects feedback from the PR #7 review).
3. ~~`max_entry_size` / `max_total_size` configuration interface~~ → **Resolved**: implemented as `pub(crate)` builder methods (`with_max_entry_size` / `with_max_total_size`), callable only from within the crate. `pipeline::run` now accepts a `SizeLimits` originating from `lib.rs` ([lib.md](../lib.en.md)) and calls both builder methods — `ZipContainer::open_reader(reader)?.with_max_entry_size(limits.max_entry_size).with_max_total_size(limits.max_total_size)` — realizing the public-API configurability security review Finding 2 called for (Issue [#14](https://github.com/MinamiyamaKotaro/xlsxparser/issues/14)).
4. ~~Tracking the archive's cumulative size~~ → **Resolved**: `ZipContainer` holds it as the `total_read` / `max_total_size` fields (the natural place, since `ZipContainer` already spans state across multiple entries). `get_entry` lends it to `BoundedReader` as `&mut u64`; no interior mutability such as `Cell` is used (reflects feedback from the PR #7 review; see [sanitize.md](sanitize.en.md) for details).
5. ~~Where the responsibility for checking required parts lives~~ → **Resolved**: `ZipContainer` stays a generic container layer whose job is "safely carve files out of a ZIP archive," with no `.xlsx` (OPC)-specific semantics (which parts are mandatory) — single responsibility principle. The existence check is handled by `pipeline.rs` / `parse/relationships.rs` reacting to `get_entry` returning `Ok(None)` (reflects feedback from the PR #7 review).
6. **Case sensitivity of entry-name lookup**: `get_entry` uses `zip::ZipArchive::by_name`, which compares case-sensitively, whereas OPC part names (ECMA-376 Part 2) are formally case-insensitive (ASCII case folding). Left case-sensitive at implementation time for simplicity — every real-world producer (Excel, Google Sheets, LibreOffice, Apache POI) keeps entry names and `.rels` `Target` references byte-identical in practice. If a non-conforming producer is found, the fix would be building a `HashMap<String, String>` (lowercased name → original name) once at `open_reader` time, alongside the existing `validate_entry_path` pass (PR #21 review).
