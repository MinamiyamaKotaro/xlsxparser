# `container/mod.rs` Design Doc

*[日本語](mod.md)*

Design doc for `src/container/mod.rs`. Implements the responsibility `architecture.md` assigns to `container/`: "the entry point for ZIP (OPC) extraction, safe file retrieval." `pipeline.rs` owns the `ZipContainer` defined here and controls resource-disposal timing across phases (architecture.md design policy 3).

## Responsibility / Scope

- Opens a ZIP (OPC) archive and reads all entry names from its central directory
- At open time, validates every entry name in one pass via [`container/sanitize.rs`](sanitize.en.md)'s `validate_entry_path`, immediately rejecting an archive that contains any invalid entry name (fail closed)
- Provides `get_entry`, the "safe file retrieval" gateway that only ever hands out a given entry's decompressed stream wrapped in the Zip-Bomb-protecting `BoundedReader` ([sanitize.md](sanitize.en.md))
- **Not responsible for**: the Zip Bomb/Zip Slip detection logic itself (`container/sanitize.rs`), XML syntax interpretation or XXE protection (`parse/`), interpreting `_rels` content or mapping sheet IDs to file paths (`parse/relationships.rs`), deciding which specific parts (e.g. `[Content_Types].xml`, `xl/workbook.xml`) are required (the caller — this file only handles "can the named entry be retrieved safely," not which parts are mandatory)

## Key Types (draft)

```rust
use crate::container::sanitize::{self, BoundedReader, DEFAULT_MAX_UNCOMPRESSED_SIZE};
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
    /// - The returned stream is wrapped in `BoundedReader`, so the Zip Bomb
    ///   cap (`max_entry_size`) is already applied.
    pub fn get_entry(&mut self, name: &str) -> Result<Option<BoundedReader<impl Read + '_>>, Error> {
        sanitize::validate_entry_path(name)?;
        let _ = name;
        unimplemented!()
    }

    /// Lists all entry names in the archive (already validated at open time).
    pub fn entry_names(&self) -> impl Iterator<Item = &str> {
        std::iter::empty()
    }
}

impl<R> ZipContainer<R> {
    /// Opens with an explicitly set uncompressed-size cap for Zip Bomb
    /// protection. When unset, `DEFAULT_MAX_UNCOMPRESSED_SIZE`
    /// ([sanitize.md](sanitize.en.md)) is assumed to apply (the concrete
    /// builder-API shape is undecided; see Open Question 3).
    fn with_max_entry_size(mut self, limit: u64) -> Self {
        self.max_entry_size = limit;
        self
    }
}
```

## Dependencies

- Depends on: [`container/sanitize.rs`](sanitize.en.md) (`validate_entry_path`, `BoundedReader`, `DEFAULT_MAX_UNCOMPRESSED_SIZE`) and [`error.rs`](../error.en.md). No dependency on `model/` or `parse/`.
- Depended on by: `pipeline.rs` only. Per architecture.md design policy 3 ("`container` and `parse` go back and forth tightly, but this call ordering and resource lifecycle management is centralized in `pipeline.rs` so other modules don't need to know about each other directly"), no module under `parse/` knows about `container::ZipContainer` directly — `pipeline.rs` passes along the byte stream it obtained via `get_entry`.

Why `get_entry` re-validates `name` on every call: the open-time application of `validate_entry_path` only covers entry names the archive itself actually holds (static strings from the central directory). But the `name` passed to `get_entry` may instead be a string that `parse/relationships.rs` (Phase 1), via `pipeline.rs`, computed dynamically by combining a relative-path notation from a `.rels` file (e.g. `../media/image1.png`) with an entry name. If that computation has any normalization gap (e.g. an unresolved `..`), a path that slipped past the open-time check could reach `get_entry`. It is therefore treated as an independent, untrusted input and validated again every time (defense in depth).

The design where `get_entry` requires `&mut self` and ties the returned value's lifetime to that borrow (`impl Read + '_`) naturally matches the sequential access pattern architecture.md already describes — "`container` and `parse` go back and forth tightly: fetch bytes → parse → fetch the next entry based on the result." It encodes in the type system the assumption that there is no need to hold multiple entries open for processing at the same time.

## Error Handling Policy

- `open` / `open_reader` return `Error::InvalidPackage` when the ZIP archive itself is corrupt. Whether the underlying ZIP crate's error is simply stringified, or held in a dedicated type-erased `Box<dyn Error>` field the way `error.md`'s `XmlParse` does, is to be revisited once the crate is chosen (Open Question 1, tied to [error.md Open Question 1](../error.en.md)).
- `open_reader` rejects the entire archive with `Error::ZipSlipDetected` if any entry name in the central directory fails `validate_entry_path` — there is no partial-acceptance fallback that uses only the "safe" entries.
- `get_entry` represents a missing entry as `Ok(None)` rather than via `Result`'s error path (the same design principle `model::Sheet::get` uses to represent a blank cell as `None`; see [model/sheet.md](../model/sheet.en.md)).
- How an `io::Error` raised mid-read from the `BoundedReader` returned by `get_entry` gets converted into `Error::ZipBombDetected` is undecided, as noted in [sanitize.md Open Question 3](sanitize.en.md).

## Testing Strategy

- Verify that `open`-ing a minimal, valid `.xlsx`-shaped ZIP succeeds and `entry_names()` returns the expected set of entries
- Verify that `open_reader`-ing corrupted ZIP bytes returns `Error::InvalidPackage`
- Verify that `open_reader`-ing a ZIP whose central directory contains an invalid entry name (e.g. `"../evil"`) returns `Error::ZipSlipDetected` and rejects the whole archive (one bad entry rejects everything)
- Verify that calling `get_entry` with a name that exists returns `Ok(Some(..))` with the expected byte content
- Verify that calling `get_entry` with a name that does not exist returns `Ok(None)` (not an error)
- Verify that calling `get_entry` with a malformed `name` such as `"../etc/passwd"` returns `Error::ZipSlipDetected` regardless of whether such an entry actually exists in the archive (a defense-in-depth test simulating a path that slipped past open-time validation)
- Verify that reading beyond `max_entry_size` from the stream `get_entry` returns produces an error (a wiring test against `sanitize::BoundedReader`; `BoundedReader`'s own logic is verified in [sanitize.md](sanitize.en.md))

## Open Questions

1. **Which external crate to use for ZIP handling**: the same question as [error.md Open Question 1](../error.en.md). Since this file's types are designed around requiring `Read + Seek`, whichever crate is chosen must support that assumption (random access over a seekable input).
2. **Return type design for `get_entry`**: currently `impl Read + '_` (RPIT, tied to `self`'s borrow), meaning a second `get_entry` call cannot be made while a previous entry's stream is still held (the borrows would conflict). This should be fine given the sequential access pattern (see Dependencies), but whether to relax the constraint with a trait object such as `Box<dyn Read + '_>` is undecided.
3. **`max_entry_size` configuration interface**: whether to use a post-hoc builder method like `with_max_entry_size`, or an argument to `open` / `open_reader`, is to be finalized together with the configurability discussion from `lib.rs`'s public API noted in [sanitize.md Open Question 1](sanitize.en.md).
4. **Tracking the archive's cumulative size**: if the "cumulative, not just per-entry" protection mentioned in [sanitize.md Open Question 2](sanitize.en.md) is implemented, whether it belongs here (`ZipContainer` is the natural place, since it already spans multiple entries' state) or as a cumulative-counter type added to `sanitize.rs` is undecided.
5. **Where the responsibility for checking required parts lives**: whether detecting that a mandatory `.xlsx` part (e.g. `[Content_Types].xml`, `xl/workbook.xml`) is missing should happen here at `open` time (making "is this a valid .xlsx package" part of this file's responsibility), or whether `pipeline.rs` / `parse/relationships.rs` should infer it from `get_entry` returning `Ok(None)`, is undecided. The former fails fast, but would give this file structural knowledge of OPC internals (which parts are mandatory), which may drift from its current scope of "only handles safe ZIP retrieval."
