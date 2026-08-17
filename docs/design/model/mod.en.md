# `model/mod.rs` Design Doc

*[日本語](mod.md)*

Design doc for `src/model/mod.rs`. This is purely an aggregation file: it declares the submodules under `model/` and re-exports the types that need to be public to the outside (`resolve/`, `json.rs`, `lib.rs`, etc.).

## Responsibility / Scope

- Declaring submodules (`mod cell; mod sheet; mod workbook;`)
- Re-exporting public types (`pub use cell::{Cell, CellValue, CellRef};` etc.)
- **Not responsible for**: type definitions themselves (the responsibility of each submodule), or any logic — per architecture.md's policy that `model/` holds only pure data structures with no logic, `mod.rs` contains no processing either.

## Key Contents (draft)

```rust
mod cell;
mod sheet;
mod workbook;

pub use cell::{Cell, CellRef, CellValue};
pub use sheet::{MergedRegion, Sheet, SheetVisibility};
pub use workbook::Workbook;
```

Where `ResolvedStyle` and `DateTimeValue` (see open questions 3 and 4 in [model/cell.md](cell.en.md)) end up being defined is not yet decided; if placed under `model/`, they would also be added to the re-exports here.

## Dependencies

- Depends on: [`model/cell.rs`](cell.en.md), [`model/sheet.rs`](sheet.en.md), [`model/workbook.rs`](workbook.en.md) (all as `mod` declarations)
- Depended on by: other layers within the crate such as `resolve/`, `json.rs`, `lib.rs` (which reference types via this file, e.g. `crate::model::Workbook`)

## Error Handling Policy

None (holds no logic, so there is nowhere for an error to be generated or propagated).

## Testing Strategy

None. Since this file only contains type definitions and re-exports, it has no unit tests of its own. Whether the intended public API surface is correct is verified through `cargo doc` output and by the build succeeding when referenced from `lib.rs`.

## Open Questions

1. **Where `ResolvedStyle` / `DateTimeValue` are defined**: As noted in open questions 3 and 4 of [model/cell.md](cell.en.md), it is undecided whether these types live under `model/` or under `resolve/style.rs`, and the re-export list here changes accordingly.
2. **Visibility scope**: Whether fields of `MergedRegion` or `CellRef` (`row` / `col`) should be made `pub` and exposed externally, or restricted to constructor-only access, is to be decided together with the public API design of `lib.rs` (a separate issue).
