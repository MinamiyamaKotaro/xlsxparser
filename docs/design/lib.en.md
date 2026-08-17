# `lib.rs` Design Doc

*[日本語](lib.md)*

Design doc for `src/lib.rs`. This is the crate root, and implements the "public API entry point" [architecture.md](architecture.en.md) defines. It declares `container/` `parse/` `resolve/` `pipeline.rs` `json.rs` as private `mod`s, hiding them as internal implementation, and re-exports outward only a subset of `model/`'s types plus `error::Error`. This is where the public API shape [pipeline.md Open Questions 1 and 2](pipeline.en.md) and [json.md Open Question 6](json.en.md) deferred to "settled when `lib.rs` is designed" gets finalized.

## Responsibility / Scope

- Defines the crate's public API functions: `parse_workbook`, which parses `.xlsx` from a file path, and `parse_workbook_reader`, which parses directly from any `Read + Seek` (both thin wrappers that internally call [`pipeline::run`](pipeline.en.md))
- `parse_workbook` backfills its own known file path into any `Error::Io { path: None, .. }` arising inside `pipeline::run` (after `File::open` succeeds, during ZIP extraction or XML streaming) before returning it to the caller (reflects the [PR #11 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/11#pullrequestreview-4949346233))
- Decides which submodules are exposed outside the crate. `container` / `parse` / `resolve` / `pipeline` / `json` are declared as private `mod`s, hiding them as crate-internal implementation. Even where an individual item is defined as `pub fn` within its own file (e.g. [`container::ZipContainer::open`](container/mod.en.md)), if the enclosing `mod` itself is private, Rust's visibility rules make it unreachable from outside the crate regardless (see Dependencies for details)
- Re-exports to the crate root, from the types `model/` defines, the ones reachable from outside via `Workbook` (`Workbook`, `Sheet`, `Cell`, `CellValue`, `CellRef`, `SheetVisibility`, `MergedRegion`, `ResolvedStyle`, `StyleId`, `DateTimeValue`), along with `error::{Error, Result}`
- Re-exports `json::{to_json_writer, to_json_string}` directly to the crate root, exposing the `Workbook`-to-JSON conversion as an independent second step (implements the resolution of [pipeline.md Open Question 1](pipeline.en.md))
- **Not responsible for**: the parsing itself (delegated to `pipeline::run`), the JSON conversion itself (delegated to `json::to_json_writer`/`to_json_string`), defining the public types themselves (`model/`)

## Key Contents (draft)

```rust
mod container;
mod error;
mod json;
mod model;
mod parse;
mod pipeline;
mod resolve;

pub use error::{Error, Result};
pub use json::{to_json_string, to_json_writer};
pub use model::{
    Cell, CellRef, CellValue, DateTimeValue, MergedRegion, ResolvedStyle, Sheet,
    SheetVisibility, StyleId, Workbook,
};

use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

/// Parses `.xlsx` from a file path — the most common public entry point.
/// A thin wrapper that opens a `std::fs::File` internally and delegates to
/// [`pipeline::run`](pipeline.en.md). Beyond a failure of `File::open`
/// itself, any I/O error arising inside `pipeline::run` (during ZIP
/// extraction or XML streaming) with `path` left unset (`None`) is
/// backfilled with the file path this function already knows before being
/// returned (reflects the [PR #11
/// review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/11#pullrequestreview-4949346233)).
pub fn parse_workbook(path: impl AsRef<Path>) -> Result<Workbook> {
    let path = path.as_ref();
    let file = File::open(path).map_err(|source| Error::Io {
        path: Some(path.to_path_buf()),
        source,
    })?;
    pipeline::run(file).map_err(|err| fill_io_path(err, path))
}

/// Backfills the file path `parse_workbook` already knows into an
/// `Error::Io { path: None, .. }` propagated from `pipeline::run`. Any
/// other variant is returned unchanged. `Error::XmlParse` /
/// `Error::MissingRequiredElement` also carry a `path` field, but theirs
/// names a part within the OPC package (e.g. `"xl/worksheets/sheet1.xml"`)
/// — a different meaning from a filesystem path — so they are excluded
/// from backfilling.
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
/// callers that don't go through the filesystem. Requiring a seekable
/// input to read the ZIP central directory simply carries forward
/// [container/mod.md](container/mod.en.md)'s `ZipContainer::open_reader`
/// constraint (a purely streaming `Read`-only input cannot be opened this
/// way).
pub fn parse_workbook_reader<R: Read + Seek>(reader: R) -> Result<Workbook> {
    pipeline::run(reader)
}
```

## Dependencies

- Depends on: [`pipeline.rs`](pipeline.en.md) (`run`), [`json.rs`](json.en.md) (`to_json_writer`, `to_json_string`), [`model/mod.rs`](model/mod.en.md) (each re-exported type), [`error.rs`](error.en.md) (`Error`, `Result`)
- Depended on by: nothing (only user code outside the crate depends on this file. No module within the crate depends on `lib.rs` — every module, including `pipeline.rs`, references `model/` directly, e.g. `crate::model::Workbook`, never going through `crate::Workbook`, the re-export path `lib.rs` provides)

Declaring `container` / `parse` / `resolve` / `pipeline` / `json` as private `mod`s relies on Rust's visibility rule that an item's effective visibility outside its own module is the narrowest of its own visibility and every enclosing module's visibility. For example, [container/mod.md](container/mod.en.md)'s `ZipContainer::open` / `get_entry` / `entry_names` are all defined as `pub fn`, but that only guarantees they can be called path-qualified (`container::ZipContainer::open(...)`) from elsewhere in the same crate, such as `pipeline.rs` — as long as `mod container;` (not `pub mod`) stays private, none of it is exposed outside the crate at all. That nearly everything under [`parse/`](parse/mod.en.md) is declared `pub(crate)` is the same conclusion written explicitly at the item level; combined with this file's private `mod` declarations, the two form a double line of defense.

[`model/mod.md`](model/mod.en.md) re-exports `StyleSheet` (the internal table type mapping a `cellXfs` index to `ResolvedStyle`) at the `model` module level via `pub use style::{ResolvedStyle, StyleId, StyleSheet};`, but this file does not re-export it any further outward. While `Cell.style: Option<Arc<ResolvedStyle>>` is a public field that makes `ResolvedStyle` alone reachable from outside, `StyleSheet` itself (the whole table) is never referenced by any field of a public type, so it stays crate-internal implementation (built by [`parse/styles.rs`](parse/styles.en.md), consumed by [`resolve/style.rs`](resolve/style.en.md)).

## Error Handling Policy

- `parse_workbook` converts a `std::fs::File::open` failure into `Error::Io { path: Some(path), source }`. It can set `path` to `Some` because this function itself holds the concrete context — a file path. This is exactly the usage [error.md](error.en.md) had in mind for the `Some` side of `Io::path: Option<PathBuf>`
- **`parse_workbook` backfills any `Error::Io { path: None, .. }` returned by `pipeline::run` via `fill_io_path`** (reflects the [PR #11 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/11#pullrequestreview-4949346233)). When `File::open` itself succeeds but an I/O error occurs later, during ZIP extraction or XML streaming (e.g. the file is deleted or corrupted mid-read), `pipeline::run` has no way of knowing which filesystem path `container::ZipContainer` etc. is reading from, so it returns `Error::Io` with `path` left as `None`. `parse_workbook` rewrites that `None` using the file path it already holds before returning to the caller, so the caller reliably gets the file name regardless of which stage of parsing the I/O error occurred in. `Error::XmlParse` / `Error::MissingRequiredElement`'s `path` field names a part within the OPC package (e.g. `"xl/worksheets/sheet1.xml"`) — a different meaning from a filesystem path — so `fill_io_path` excludes them from backfilling
- `parse_workbook_reader` generates no I/O error itself (`reader` is already an in-memory or caller-provided input; this function performs no opening step on it). Any error arising within `pipeline::run` simply propagates via `?`. Any `Error::Io` generated there is never backfilled and stays `path: None` (unlike `parse_workbook`, this function never had a file path in the first place, so there is nothing to backfill with) — this is exactly the "input that doesn't go through a file path" case [error.md](error.en.md) had already anticipated when designing `Io::path: Option<PathBuf>`
- This file itself never generates a new `Error` variant. It simply returns existing variants (everything besides the `Io` case above propagates up from `pipeline::run` and below) straight to the caller. `fill_io_path` only rewrites the `path` field of an existing `Error::Io` instance; it never constructs a new variant

## Testing Strategy

- Verify that passing a valid `.xlsx` file path to `parse_workbook` returns `Ok(Workbook)` (a filesystem-backed integration test)
- Verify that passing a non-existent path to `parse_workbook` returns `Error::Io { path: Some(path), .. }` (including that `path` is set correctly)
- **Unit-test `fill_io_path` directly: verify that passing `Error::Io { path: None, .. }` rewrites `path` to `Some`, and that `Error::Io { path: Some(..), .. }` or any other variant (e.g. `Error::XmlParse`) passes through unchanged** (a unit test for the backfill behavior added by the PR #11 review; an integration test that actually reproduces an I/O error mid-read inside `pipeline::run` would depend on filesystem-operation timing and tend to be flaky, so a direct unit test of `fill_io_path` is used instead)
- Verify that passing a `std::io::Cursor<Vec<u8>>` holding valid `.xlsx`-shaped bytes to `parse_workbook_reader` returns `Ok(Workbook)`
- Verify that `parse_workbook` (via a file) and `parse_workbook_reader` (in-memory) return the same `Workbook` for identical `.xlsx` data (a wiring test confirming both functions are simple delegations to `pipeline::run`)
- Verify that passing `parse_workbook`'s return value directly into `to_json_string` yields a valid JSON string (an end-to-end test verifying the two-step public API actually chains together)
- Verify that for a corrupt `.xlsx` (invalid ZIP, missing required parts, etc.), the `Error` variants defined across [`pipeline.md`](pipeline.en.md) propagate unchanged all the way to the caller (confirming `lib.rs` itself never swallows or converts them into a different error)
- Verify that the public types (`Workbook`, `Sheet`, `Cell`, `CellValue`, `CellRef`, `SheetVisibility`, `MergedRegion`, `ResolvedStyle`, `StyleId`, `DateTimeValue`, `Error`, `Result`) are reachable from outside the crate as names directly under `xlsxparser::` (a doctest, or a test pinning the public API surface — see Open Questions for the concrete method)
- Verify that types under `container` / `parse` / `resolve` / `pipeline` / `json` (e.g. `ZipContainer`, `SharedStringTable`) are unreachable from outside the crate (a compile error). Since an ordinary unit test cannot itself verify "this fails to compile," consider adopting a compile-failure-verification crate such as `trybuild` (see Open Question 3)

## Open Questions

1. **Whether a one-shot JSON convenience function is warranted**: currently exposes only the two-step call `parse_workbook` → `to_json_string`, with no convenience function that does both in one call (e.g. `parse_workbook_json(path) -> Result<String>`). If most use cases ultimately just want JSON, whether adding such a convenience function is worthwhile is a question worth revisiting together with a more detailed elaboration of the requirements' frontend use case.
2. ~~Crate name / package name~~ → **Resolved**: `xlsxparser` (set in `Cargo.toml` when the crate skeleton was scaffolded for CI, Issue #16, ahead of this file's implementation).
3. **How to verify private-module types never leak outward**: still undecided; not addressed by this file's initial implementation. `crate::container::ZipContainer` and similar are unreachable from outside the crate today purely because every module that could re-export them keeps them at `pub(crate)`/private-`mod` visibility — verified so far only by manual review (Dependencies section), not by an automated compile-failure or public-API-diff check. `trybuild` / `cargo public-api` remain candidates if this needs to be enforced automatically later.
4. ~~Field-level visibility of `Sheet` / `Cell`, etc.~~ → **Resolved**: the existing `pub` field granularity (e.g. `MergedRegion`/`CellRef`'s `row`/`col` fields, `Cell`'s `value`/`style`) was carried forward as-is into the implementation, as this file's design anticipated.
5. **Whether `no_std` support is needed**: both `parse_workbook`/`parse_workbook_reader` depend on `std::fs::File` and `std::io::{Read, Seek}`. The requirements have no `no_std`-environment requirement, so this is out of scope for now — but given how heavily `container/` and `parse/`'s designs already depend on `std::io`, supporting it later would require a substantial redesign.
