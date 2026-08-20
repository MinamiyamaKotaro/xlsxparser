# `model/mod.rs` Design Doc

*[日本語](mod.md)*

Design doc for `src/model/mod.rs`. This is purely an aggregation file: it declares the submodules under `model/` and re-exports the types that need to be public to the outside (`resolve/`, `json.rs`, `lib.rs`, etc.).

## Responsibility / Scope

- Declaring submodules (`mod cell; mod sheet; mod workbook; mod style; mod color;`)
- Re-exporting public types (`pub use cell::{Cell, CellValue, CellRef};` etc.)
- **Not responsible for**: type definitions themselves (the responsibility of each submodule), or any logic — per architecture.md's policy that `model/` holds only pure data structures with no logic, `mod.rs` contains no processing either.

## Key Contents (draft)

```rust
mod cell;
mod sheet;
mod workbook;
mod style;
mod color;

pub use cell::{Cell, CellRef, CellValue};
pub use sheet::{MergedRegion, Sheet, SheetVisibility};
pub use workbook::Workbook;
pub use style::{Alignment, ColorRef, Font, ResolvedStyle, StyleId, StyleSheet};
pub use color::{Rgb, ThemePalette};
```

`Rgb`/`ThemePalette` ([`model/color.rs`](color.en.md), Issue #76) are re-exported as part of the public API surface reachable from `Workbook::theme()`/`ResolvedStyle.fill_fg_color` and similar — calling [`resolve::color::resolve_color`](../resolve/color.en.md) directly from outside the crate (the "Option A" call shape, see [resolve/color.md](../resolve/color.en.md)) is not possible without these types being public.

`DateTimeValue` (see open question 4 in [model/cell.md](cell.en.md)) is already defined inside `model/cell.rs`, so it is included in the `cell::{..}` re-export. [`lib.md`](../lib.en.md)'s design settled that re-exporting `DateTimeValue` is mandatory, since `CellValue::DateTime` is part of the crate's public API (the concrete type itself remains a separate matter, handled by [model/cell.md Open Question 4](cell.en.md)). Where `ResolvedStyle` / `StyleSheet` / `StyleId` live has now been settled by adding [`model/style.rs`](style.en.md) (resolves the former Open Question 1 — addresses PR #8 review feedback).

## Dependencies

- Depends on: [`model/cell.rs`](cell.en.md), [`model/sheet.rs`](sheet.en.md), [`model/workbook.rs`](workbook.en.md), [`model/style.rs`](style.en.md), [`model/color.rs`](color.en.md) (all as `mod` declarations)
- Depended on by: other layers within the crate such as `resolve/`, `parse/`, `json.rs`, `lib.rs` (which reference types via this file, e.g. `crate::model::Workbook`)

## Error Handling Policy

None (holds no logic, so there is nowhere for an error to be generated or propagated).

## Testing Strategy

None. Since this file only contains type definitions and re-exports, it has no unit tests of its own. Whether the intended public API surface is correct is verified through `cargo doc` output and by the build succeeding when referenced from `lib.rs`.

## Open Questions

1. ~~Where `ResolvedStyle` is defined~~ → **Resolved**: newly added [`model/style.rs`](style.en.md) and defined it there (addresses PR #8 review feedback). `DateTimeValue` has always been a placeholder defined inside `model/cell.rs`; its location was never in question (its concrete type is a separate matter, handled by [model/cell.md Open Question 4](cell.en.md)).
2. ~~Visibility scope~~ → **Resolved**: [`lib.md`](../lib.en.md)'s design settled that `model/`'s main types themselves (`Workbook`/`Sheet`/`Cell`/`CellValue`/`CellRef`/`SheetVisibility`/`MergedRegion`/`ResolvedStyle`/`StyleId`/`DateTimeValue`) are re-exported outward. `MergedRegion`/`CellRef`'s `row`/`col` fields are carried forward as `pub`, unchanged from their existing type definitions (see [lib.md Open Question 4](../lib.en.md)). `StyleSheet` is never reachable from any public type's field (e.g. `Cell`'s), so it is not re-exported and stays crate-internal implementation (see [lib.md Dependencies](../lib.en.md)).
