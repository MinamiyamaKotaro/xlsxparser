# `model/color.rs` Design Doc

*[日本語](color.md)*

Design doc for `src/model/color.rs`. Defines the type representing an actual, resolved RGB value, needed by [Issue #76](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76) (converting theme/indexed colors to real RGB values, a display-oriented concern). This is a newly added file, independent of [`model/style.rs`](style.en.md)'s `ColorRef` (which keeps `rgb`/`theme`+`tint`/`indexed` raw and never resolves it to a real RGB value — [Issue #75](https://github.com/MinamiyamaKotaro/xlsxparser/issues/75)'s diff-oriented scope), and exists purely for the display use case. Defines only pure, logic-free data structures (the same role [model/style.md](style.en.md) plays).

[`parse/theme.rs`](../parse/theme.en.md) (the entity that builds a `ThemePalette` from `theme{N}.xml`) and [`resolve/color.rs`](../resolve/color.en.md) (the entity that resolves a `ColorRef` and `ThemePalette` into a real RGB value) are connected indirectly, only through the types defined here, without knowing about each other directly — the same "shared vocabulary between phases" role [model/style.md](style.en.md) plays between `parse/styles.rs` and `resolve/style.rs`.

## Responsibility / Scope

- Defines `Rgb`, a lightweight `Copy` type for a real RGB value
- Defines `ThemePalette`, which holds the 12 colors from `theme{N}.xml`'s `<clrScheme>`
- **Not responsible for**: XML parsing of `theme{N}.xml` itself ([`parse/theme.rs`](../parse/theme.en.md), not yet designed), the logic that resolves a `ColorRef` to an `Rgb` (including tint correction and indexed-palette lookup — [`resolve/color.rs`](../resolve/color.en.md), not yet designed), the `ColorRef` type definition itself ([`model/style.rs`](style.en.md))

## Key Types (draft)

```rust
/// A real RGB value. A `Copy` type that fits within a 4-byte boundary with
/// no heap allocation. Carries no alpha channel — whether a cell's fill is
/// actually visible is controlled by `patternType` (`none`/`solid`/...),
/// not by any notion of color transparency. The leading two hex digits of
/// `ColorRef::Rgb` (an 8-digit ARGB string like `"FFFF0000"`) are, in
/// practice, almost always `FF` (opaque) as well — when
/// [`resolve/color.rs`](../resolve/color.en.md) resolves it to an `Rgb`, it
/// simply discards them (see Open Question 1 below).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// The 12 colors `theme{N}.xml`'s `<clrScheme>` defines. Held as a
/// fixed-size array that can live on the stack, with zero heap allocation
/// ([Issue #76 design proposal](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)'s
/// top priority — "no CPU regression, no added memory footprint" —
/// carried through directly).
///
/// **Careful**: the array index is *not* the `<clrScheme>` XML declaration
/// order (`dk1, lt1, dk2, lt2, accent1..6, hlink, folHlink`). The index
/// `styles.xml`'s `theme` attribute refers to has slots 0/1 swapped:
/// `lt1, dk1, lt2, dk2, accent1..6, hlink, folHlink` (matching the order
/// Apache POI's `ThemesTable.ThemeElement` enum uses, confirmed against
/// real data by a PoC — see
/// [Issue #76 comment](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352366260)).
/// This is a well-known trap easy to get wrong, so
/// [`parse/theme.rs`](../parse/theme.en.md) owns absorbing this swap when
/// it builds this array — this file itself only documents the index
/// convention as a contract ("the array is stored in this order"); it
/// carries no logic to perform the swap itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemePalette(pub [Rgb; 12]);
```

`Rgb` derives `Default` (black, `#000000`) because part of [`resolve/color.rs`](../resolve/color.en.md)'s fallback path (the `sysClr` fallback policy finalized in [Issue #76 comment](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352388486)) uses black as its default value. `ThemePalette` does not derive `Default` — it only carries meaning once all 12 slots are filled, and allowing an empty/partially-initialized `ThemePalette` to be constructed would make `resolve/color.rs`'s calling contract ambiguous.

## Dependencies

- Depends on: nothing (no dependency on sibling modules within `model/` either — the same leaf-module status as [`model/style.rs`](style.en.md))
- Depended on by: [`parse/theme.rs`](../parse/theme.en.md) (builds `ThemePalette`), [`resolve/color.rs`](../resolve/color.en.md) (reads `Rgb`/`ThemePalette` and resolves a `ColorRef` to an `Rgb`), [`model/workbook.rs`](workbook.en.md) (holds it as `Workbook.theme: Option<ThemePalette>` — see Open Question 2 below and [workbook.md](workbook.en.md))

## Error Handling Policy

Not applicable (same as [`model/style.rs`](style.en.md) — this file defines only pure, logic-free data structures). Erroring on `theme{N}.xml` parse failure or slot omission is [`parse/theme.rs`](../parse/theme.en.md)'s responsibility.

## Test Policy

Not applicable. Type definitions only, so this file has no unit tests. Whether `ThemePalette`'s index convention (the slot 0/1 swap) is actually honored is verified on the [`parse/theme.rs` Test Policy](../parse/theme.en.md) side.

## Open Questions

1. **Whether discarding the alpha channel from `ColorRef::Rgb`'s 8-digit ARGB string is sound**: confirmed via PoC against the fixtures on hand that it's practically always `FF` (opaque), but if a real file with a non-`FF` alpha value surfaces in the future, it remains an open question whether silently ignoring it (the current policy) is acceptable, or whether `Rgb` should gain an alpha field. The cost of adding one later is low (`Rgb` isn't locked into any public API surface yet), so this is deferred until a concrete example turns up.
2. **How `Workbook` holds a `ThemePalette`**: for [Issue #76 design proposal](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)'s "Option A (on-demand resolve API)" to actually work, callers need access to the workbook's `ThemePalette` in addition to a `ColorRef` — the original proposal's module layout didn't spell out this path, so this design pass filled the gap by adding a `theme: Option<ThemePalette>` field to [`model/workbook.rs`](workbook.en.md) (see [workbook.md](workbook.en.md) for detail). `Option` because a workbook without a `theme{N}.xml` part (the vast majority of files, which never use theme colors) has none.
