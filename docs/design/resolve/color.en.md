# `resolve/color.rs` Design Doc

*[日本語](color.md)*

Design doc for `src/resolve/color.rs`. Handles the resolution logic [Issue #76](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76) (converting theme/indexed colors to real RGB values) requires. Provides pure functions that compute the real, displayed `Rgb` from a `ColorRef` (a raw color reference, defined by [`model/style.rs`](../model/style.en.md)) and a `ThemePalette` (a theme's 12 colors, defined by [`model/color.rs`](../model/color.en.md)).

Per [architecture.md](../architecture.en.md) design principle 2 ("`resolve/` is independent of I/O and style resolution, and is self-contained using only in-memory data structures"), this file never touches XML parsing — it doesn't know [`parse/theme.rs`](../parse/theme.en.md) (which builds `ThemePalette`) directly, connecting to it only indirectly through [`model/color.rs`](../model/color.en.md)'s types (the same shape as the [`resolve/style.rs`](style.en.md)/`parse/styles.rs` relationship).

## Responsibility / Scope

- Provides `apply_tint`, a pure function that applies HSL luminance correction via `tint` (a self-contained sRGB→HSL→luminance-correction→sRGB transform; not fill-color-specific, so it's reusable if font/border colors ever need theme-color support)
- Provides `lookup_indexed_color`, which resolves a reference into ECMA-376's legacy fixed 64-color palette into a real RGB value. Resolves `indexed=64` (System Foreground)/`65` (System Background) as OS-independent, deterministic fixed values (`64→#000000`, `65→#FFFFFF` — the finalized policy from [Issue #76 comment](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352388486); this crate runs headless, so it can't depend on an OS system palette)
- Provides `resolve_color`, the entry point that resolves any of [`model::style::ColorRef`](../model/style.en.md)'s three variants (`Rgb`/`Theme`/`Indexed`) to a real RGB value (the substance of [Issue #76 design proposal](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)'s "Option A: on-demand resolve API." Meant to be called only where a display-oriented caller actually needs it, never computed unconditionally at parse time or during style resolution [Phase 2/4] — `ResolvedStyle`'s own memory layout does not grow by a single byte)
- **Not responsible for**: the `Rgb`/`ThemePalette` type definitions themselves ([`model/color.rs`](../model/color.en.md)), the `ColorRef` type definition itself ([`model/style.rs`](../model/style.en.md)), XML parsing of `theme{N}.xml` ([`parse/theme.rs`](../parse/theme.en.md)), whether to read the `theme{N}.xml` part at all (`pipeline.rs`, see [pipeline.md Open Question 6](../pipeline.en.md))

## Key Types / Functions (draft)

```rust
use crate::model::color::{Rgb, ThemePalette};
use crate::model::style::ColorRef;

/// ECMA-376's legacy fixed 64-color palette (indexed=0..=63). A
/// compile-time constant array embedded in the binary, zero runtime
/// memory cost ([Issue #76 design proposal](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)).
/// A PoC confirmed all 64 entries match both the value a bundled fixture
/// re-declares as its own `<colors><indexedColors>`, and
/// `openpyxl.styles.colors.COLOR_INDEX`
/// ([Issue #76 comment](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352366260)).
const INDEXED_PALETTE: [Rgb; 64] = [
    // ECMA-376's default 64 colors. Values already cross-checked against
    // real data at PoC time — transcribed here again at implementation
    // time (the PoC code itself lives under `poc/` and is not committed
    // to the repository).
];

/// Applies `tint` luminance correction to a base color from
/// `theme{N}.xml`'s `<clrScheme>`. Returns `base` unchanged when `tint`
/// is `0.0` or non-finite (`NaN`/`Inf`) — a safe degradation against a
/// crafted `tint` value (`tint="nan"`, etc — see
/// [Issue #76 design proposal](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)'s
/// security/robustness section).
///
/// Formula (ECMA-376's luminance-correction algorithm, confirmed against
/// Apache POI's implementation and multiple independent sources — see
/// [Issue #76 comment](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352366260)):
/// when `tint > 0`, `l' = l*(1-tint) + tint` (lighten); when `tint < 0`,
/// `l' = l*(1+tint)` (darken). A PoC confirmed a Rust implementation and
/// an independent Python `colorsys` re-implementation produce identical
/// results (`#4F81BD` + tint -0.25 → `#376092`).
pub(crate) fn apply_tint(base: Rgb, tint: f64) -> Rgb {
    let _ = (base, tint);
    unimplemented!()
}

/// Converts sRGB to HSL. Internal helper used only by `apply_tint`.
fn rgb_to_hsl(c: Rgb) -> (f64, f64, f64) {
    let _ = c;
    unimplemented!()
}

/// Converts HSL to sRGB. Internal helper used only by `apply_tint`.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> Rgb {
    let _ = (h, s, l);
    unimplemented!()
}

/// Resolves a legacy indexed color (the `indexed` attribute) to a real
/// RGB value. `0..=63` is a straightforward lookup into
/// `INDEXED_PALETTE`. `64`/`65` are special values representing the
/// system foreground/background colors, resolved as OS-independent,
/// deterministic fixed colors (`64→black`, `65→white` — see
/// [Issue #76 comment](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352388486)).
/// `66` and above are out of range and return `None` rather than
/// panicking.
pub(crate) fn lookup_indexed_color(index: u32) -> Option<Rgb> {
    match index {
        0..=63 => Some(INDEXED_PALETTE[index as usize]),
        64 => Some(Rgb { r: 0x00, g: 0x00, b: 0x00 }),
        65 => Some(Rgb { r: 0xFF, g: 0xFF, b: 0xFF }),
        _ => None,
    }
}

/// Resolves the real RGB value a `ColorRef` refers to. `theme` is `Some`
/// only when the workbook has a `theme{N}.xml` part
/// ([`model::Workbook::theme`](../model/workbook.en.md); `None` for a
/// workbook without the part at all).
///
/// - `ColorRef::Rgb(s)`: parses the lower 6 hex digits of `s` (an 8-digit
///   ARGB string) as RGB. The alpha digits are discarded (see
///   [model/color.md Open Question 1](../model/color.en.md)). Returns
///   `None` if `s` isn't valid 6-/8-digit hex (`ColorRef::Rgb` never
///   validates the value at parse time and keeps it verbatim — see
///   [model/style.md](../model/style.en.md)).
/// - `ColorRef::Theme { index, tint }`: returns `None` if `theme` is
///   `None` (no theme part) or `index` is out of `0..=11`. Otherwise
///   looks up the base color from `theme` and applies `apply_tint` if
///   `tint` is `Some`.
/// - `ColorRef::Indexed(index)`: delegates straight to
///   `lookup_indexed_color`.
///
/// No branch ever panics — invalid or crafted input degrades safely to
/// `None` (see
/// [Issue #76 design proposal](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)'s
/// security/robustness section).
pub fn resolve_color(color: &ColorRef, theme: Option<&ThemePalette>) -> Option<Rgb> {
    let _ = (color, theme);
    unimplemented!()
}
```

Why `resolve_color` is a free function rather than an inherent method on `ColorRef` (`color.resolve(theme)`): [Issue #76 design proposal](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)'s original draft envisioned an inherent `ColorRef::resolve(&self, ...)` method, but `ColorRef` is defined in [`model/style.rs`](../model/style.en.md), and [architecture.md](../architecture.en.md) constrains `model/` to "pure, logic-free data structures only" (already made explicit in [model/mod.md](../model/mod.en.md)'s "Not responsible for"). This is kept consistent with the existing design choice that [`resolve/style.rs`](style.en.md)'s `resolve()` is a free function rather than an inherent method on `ResolvedStyle`, carrying that same "no logic on `model/` types" policy straight through.

## Dependencies

- Depends on: [`model/color.rs`](../model/color.en.md) (`Rgb`, `ThemePalette`), [`model/style.rs`](../model/style.en.md) (`ColorRef`)
- Depended on by: `json.rs` (in the future, only if display-oriented JSON output is ever required — not called today; see [json.md Open Question 4](../json.en.md)), external callers outside the crate (`resolve_color` is `pub`; expected to be called directly with a `ColorRef` obtained from `Workbook`/`ResolvedStyle` and `Workbook::theme` — exactly the "call only where actually needed" usage "Option A" intends)

Not called from `resolve/mod.rs`'s `resolve_sheet` (Phase 4's entry point) — the `ColorRef` [`resolve/style.rs`](style.en.md) applies to a cell stays raw, and resolving it to a real RGB value happens independently of cell traversal, at whatever time the caller opts into ([Issue #76 design proposal](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)'s "per-style/per-call, not per-cell" policy).

## Error Handling Policy

- None of `apply_tint`/`lookup_indexed_color`/`resolve_color` return `Result` — every failing branch degrades to `None` (or, for `apply_tint`, the identity transform of returning `base` unchanged) — panicking is never acceptable against a crafted/corrupt `.xlsx` (an out-of-range `theme` index, a non-finite `tint`, an out-of-range `indexed` value; implements the security/robustness section of [Issue #76 design proposal](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575))
- This "degrade to `None` instead of propagating an error to the caller" policy extends [`resolve/style.rs`'s Error Handling Policy](style.en.md) ("a gentle failure interpreting an individual value is never an error unless it compromises the document's overall integrity") — but unlike `resolve/style.rs::resolve`, this file's functions were never designed to return `Result` in the first place. `resolve_color` isn't part of Phase 4's pipeline; it's an API the caller invokes at a time of its own choosing, so there's no reason to treat "couldn't resolve" as an error — the fact that the display color couldn't be determined is already fully expressed by `None`

## Test Policy

- **`apply_tint`**: confirm `base` is unchanged at `tint=0.0`; confirm `base` is unchanged (no panic) at `tint=NaN`/`tint=Infinity`; confirm convergence to pure white (`#FFFFFF`) at `tint=1.0` and pure black (`#000000`) at `tint=-1.0` (boundary values); confirm applying `tint=-0.25` to `accent1(#4F81BD)` yields `#376092` (a regression test for the concrete value PoC-verified against real data and an independent re-implementation — [Issue #76 comment](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352366260))
- **`lookup_indexed_color`**: confirm `0`/`63` (both ends of the range) resolve to `INDEXED_PALETTE`'s corresponding values; confirm `64` resolves to `#000000` and `65` to `#FFFFFF`; confirm `66` and `u32::MAX` resolve to `None` (no panic)
- **`resolve_color`**: confirm `ColorRef::Rgb("FFFF0000")` resolves to `Rgb{r:0xFF,g:0x00,b:0x00}`; confirm a `ColorRef::Rgb` holding invalid hex resolves to `None`
- **`resolve_color`**: confirm `ColorRef::Theme{index:4,tint:Some(-0.25)}` resolves to the value obtained by applying `apply_tint` to the corresponding `ThemePalette` entry; confirm the base color is returned unchanged when `tint:None`
- **`resolve_color`**: confirm `None` is returned when resolving a `ColorRef::Theme` against `theme:None` (a workbook with no theme part) — safe degradation when no theme is present
- **`resolve_color`**: confirm a `ColorRef::Theme` with `index >= 12` resolves to `None` — safe degradation for an out-of-range index
- **`resolve_color`**: confirm `ColorRef::Indexed(64)`/`ColorRef::Indexed(200)` resolve to the same results as `lookup_indexed_color` (`Some(#000000)`/`None` respectively) — confirming the delegation is wired correctly

## Open Questions

1. **Integration with display-oriented output in `json.rs`**: whether to include a resolved RGB value on `ResolvedStyle` or `JsonCell` — the "Option B" [Issue #76 design proposal](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575) touched on — is deferred until a concrete downstream use case actually needs it. For now, only "Option A" is implemented: crate consumers call `resolve_color` directly.
2. **`INDEXED_PALETTE`'s concrete values**: the draft above omits them (the PoC code itself lives under `poc/` and isn't committed to the repository). At implementation time, transcribe the same 64 values already PoC-verified in [Issue #76 comment](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352366260).
