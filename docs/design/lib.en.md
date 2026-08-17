# `lib.rs` Design Doc

*[日本語](lib.md)*

Design doc for `src/lib.rs`. This is the crate root, and implements the "public API entry point" [architecture.md](architecture.en.md) defines. It declares `container/` `parse/` `resolve/` `pipeline.rs` `json.rs` as private `mod`s, hiding them as internal implementation, and re-exports outward only a subset of `model/`'s types plus `error::Error`. This is where the public API shape [pipeline.md Open Questions 1 and 2](pipeline.en.md) and [json.md Open Question 6](json.en.md) deferred to "settled when `lib.rs` is designed" gets finalized.

## Responsibility / Scope

- Defines the crate's public API functions: `parse_workbook`, which parses `.xlsx` from a file path, and `parse_workbook_reader`, which parses directly from any `Read + Seek` (both thin wrappers that internally call [`pipeline::run`](pipeline.en.md))
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
/// [`pipeline::run`](pipeline.en.md).
pub fn parse_workbook(path: impl AsRef<Path>) -> Result<Workbook> {
    let path = path.as_ref();
    let file = File::open(path).map_err(|source| Error::Io {
        path: Some(path.to_path_buf()),
        source,
    })?;
    pipeline::run(file)
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
- `parse_workbook_reader` generates no I/O error itself (`reader` is already an in-memory or caller-provided input; this function performs no opening step on it). Any error arising within `pipeline::run` (e.g. `container::ZipContainer::open_reader` detecting a corrupt byte sequence when trying to read it as a ZIP) simply propagates via `?`. Any `Error::Io` generated there has `path: None` — this is exactly the "input that doesn't go through a file path, or a future `Read`-trait input `lib.rs` accepts" case [error.md](error.en.md) had already anticipated when designing `Io::path: Option<PathBuf>`
- This file itself never generates a new `Error` variant. It simply returns existing variants (everything besides the `Io` case above propagates up from `pipeline::run` and below) straight to the caller

## Testing Strategy

- Verify that passing a valid `.xlsx` file path to `parse_workbook` returns `Ok(Workbook)` (a filesystem-backed integration test)
- Verify that passing a non-existent path to `parse_workbook` returns `Error::Io { path: Some(path), .. }` (including that `path` is set correctly)
- Verify that passing a `std::io::Cursor<Vec<u8>>` holding valid `.xlsx`-shaped bytes to `parse_workbook_reader` returns `Ok(Workbook)`
- Verify that `parse_workbook` (via a file) and `parse_workbook_reader` (in-memory) return the same `Workbook` for identical `.xlsx` data (a wiring test confirming both functions are simple delegations to `pipeline::run`)
- Verify that passing `parse_workbook`'s return value directly into `to_json_string` yields a valid JSON string (an end-to-end test verifying the two-step public API actually chains together)
- Verify that for a corrupt `.xlsx` (invalid ZIP, missing required parts, etc.), the `Error` variants defined across [`pipeline.md`](pipeline.en.md) propagate unchanged all the way to the caller (confirming `lib.rs` itself never swallows or converts them into a different error)
- Verify that the public types (`Workbook`, `Sheet`, `Cell`, `CellValue`, `CellRef`, `SheetVisibility`, `MergedRegion`, `ResolvedStyle`, `StyleId`, `DateTimeValue`, `Error`, `Result`) are reachable from outside the crate as names directly under `xlsxparser::` (a doctest, or a test pinning the public API surface — see Open Questions for the concrete method)
- Verify that types under `container` / `parse` / `resolve` / `pipeline` / `json` (e.g. `ZipContainer`, `SharedStringTable`) are unreachable from outside the crate (a compile error). Since an ordinary unit test cannot itself verify "this fails to compile," consider adopting a compile-failure-verification crate such as `trybuild` (see Open Question 3)

## Open Questions

1. **Whether a one-shot JSON convenience function is warranted**: currently exposes only the two-step call `parse_workbook` → `to_json_string`, with no convenience function that does both in one call (e.g. `parse_workbook_json(path) -> Result<String>`). If most use cases ultimately just want JSON, whether adding such a convenience function is worthwhile is a question worth revisiting together with a more detailed elaboration of the requirements' frontend use case.
2. **Crate name / package name**: this crate's name (`Cargo.toml`'s `name`, the identifier referred to as `xlsxparser::` in code examples) is specified nowhere in the requirements or architecture.md and remains undecided. To be settled when `Cargo.toml` is set up.
3. **How to verify private-module types never leak outward**: how to automatically verify that the "hiding via private `mod` declarations" discussed under Dependencies actually works as intended is undecided. Candidates include adopting `trybuild` (tests that expect a compile failure) or diffing against a public-API snapshot via a tool like `cargo public-api`; neither has precedent in this library, so whether to adopt either is to be settled when `Cargo.toml` is set up.
4. **Field-level visibility of `Sheet` / `Cell`, etc.**: the point [model/mod.md Open Question 2](model/mod.en.md) had deferred to "settled together with `lib.rs`'s public API design." This file's design settles the premise that "`model/`'s main types themselves are exposed outward"; what remains is only confirming whether the granularity already committed as `pub` fields (e.g. `MergedRegion`/`CellRef`'s `row`/`col` fields) is sound as the final public API (the judgment here is that the existing type definitions can simply be carried forward as-is).
5. **Whether `no_std` support is needed**: both `parse_workbook`/`parse_workbook_reader` depend on `std::fs::File` and `std::io::{Read, Seek}`. The requirements have no `no_std`-environment requirement, so this is out of scope for now — but given how heavily `container/` and `parse/`'s designs already depend on `std::io`, supporting it later would require a substantial redesign.
