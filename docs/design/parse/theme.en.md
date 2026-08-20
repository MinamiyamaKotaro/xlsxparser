# `parse/theme.rs` Design Doc

*[日本語](theme.md)*

Design doc for `src/parse/theme.rs`. Handles the `parse/` responsibility [architecture.md](../architecture.en.md) defines for "parsing `theme{N}.xml`" ([Issue #76](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76)). Parses `xl/theme/theme{N}.xml`'s `<a:clrScheme>` and builds the `ThemePalette` [`model/color.rs`](../model/color.en.md) defines.

## Responsibility / Scope

- Streams only the 12 elements directly under `<a:clrScheme>` (`dk1`/`lt1`/`dk2`/`lt2`/`accent1`-`accent6`/`hlink`/`folHlink`), reading the real RGB value out of each one's child `<a:srgbClr val="RRGGBB"/>` or `<a:sysClr val="..." lastClr="RRGGBB"/>`. Anything outside `<clrScheme>` (shape styles, font schemes, ...) is never interpreted — exactly the scope [Issue #76 design proposal](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575) asks for
- Places the 12 colors read into the index order [`model::color::ThemePalette`](../model/color.en.md) contracts (`0:lt1, 1:dk1, 2:lt2, 3:dk2, 4..=9:accent1..=6, 10:hlink, 11:folHlink` — slots 0/1 swapped relative to the XML declaration order `dk1,lt1,...`). This swap was confirmed against real data and against Apache POI's `ThemesTable.ThemeElement` enum by a PoC ([Issue #76 comment](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352366260))
- An element like `<a:sysClr val="windowText" lastClr="000000"/>` uses its `lastClr` attribute (the cached value Excel writes on save) as the real RGB value. When `lastClr` is missing or not valid hex, it degrades to a slot-name-dependent fallback (`lt1`/`lt2` → white, `dk1`/`dk2`/anything else → black — the finalized policy from [Issue #76 comment](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352388486)). A PoC scan of every bundled fixture (84 `<a:sysClr>` elements total) found `lastClr` missing in exactly none of them — real-world Excel-generated files always populate it. This fallback exists purely as defense against a crafted/corrupt file and never fires on the normal path
- **Not responsible for**: the `Rgb`/`ThemePalette` type definitions themselves ([`model/color.rs`](../model/color.en.md)), `tint` correction and legacy 64-color indexed-palette resolution ([`resolve/color.rs`](../resolve/color.en.md), not yet designed — `tint` never appears in `theme{N}.xml` itself, it's attached per-reference-site in `styles.xml`, so it isn't information this file even has), resolving the actual on-disk path of the `theme{N}.xml` part (relationship resolution from `xl/_rels/workbook.xml.rels`; `pipeline.rs`, see [pipeline.md Open Question 6](../pipeline.en.md) — this function assumes it's handed an already-resolved `reader`, the same shape `parse/styles.rs`/`parse/shared_strings.rs` take), and whether to read the `theme{N}.xml` part at all ("pay-for-what-you-use" — skipping this parse entirely when `StyleSheet` contains no `ColorRef::Theme` at all is the caller `pipeline.rs`'s responsibility, see [pipeline.md Open Question 6](../pipeline.en.md))

## Key Types / Functions (draft)

```rust
use crate::error::Error;
use crate::model::color::{Rgb, ThemePalette};
use crate::parse::{convert_xml_error, create_secure_reader, optional_attr};
use quick_xml::events::Event;
use std::io::BufRead;

/// The 12 named slots `<clrScheme>` has. Listed not in declaration order
/// but in the resolved index order `ThemePalette` contracts (0:lt1, 1:dk1,
/// ...) — this table itself doubles as the "name -> output index" mapping.
const SLOT_NAMES: [&str; 12] = [
    "lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4",
    "accent5", "accent6", "hlink", "folHlink",
];

/// Parses `xl/theme/theme{N}.xml` and builds a `ThemePalette`. `path` is
/// an already-resolved part path (the caller's responsibility, see
/// Responsibility / Scope).
pub(crate) fn parse_theme(reader: impl BufRead, path: &str) -> Result<ThemePalette, Error> {
    let mut xml_reader = create_secure_reader(reader);
    // Implementation plan:
    // 1. Stream the 12 elements directly under <a:clrScheme>, matching
    //    each against SLOT_NAMES by local_name() (namespace prefix
    //    ignored). The schema fixes <clrScheme>'s child order to the
    //    declaration order (dk1,lt1,dk2,lt2,...), but this parser matches
    //    by name so it doesn't depend on that order (see Open Question 1).
    // 2. For each slot, read the RGB value out of its child
    //    <a:srgbClr val="RRGGBB"/> or
    //    <a:sysClr val="windowText" lastClr="RRGGBB"/>, delegating to
    //    resolve_slot_color.
    // 3. If any of the 12 slots was never found by the end (either
    //    <clrScheme> itself is missing, or a subset of its children are),
    //    return Error::MissingRequiredElement — unlike a missing numFmtId,
    //    ThemePalette is a fixed-size 12-element array that only carries
    //    meaning once fully populated, so a partial build is not allowed
    //    (see Error Handling Policy).
    // 4. Once all 12 slots resolve, return a ThemePalette wrapping a
    //    [Rgb; 12] in exactly SLOT_NAMES' order (= the index order
    //    ThemePalette contracts).
    let _ = (&mut xml_reader, path);
    unimplemented!()
}

/// Resolves one slot's color element (`<a:srgbClr>` or `<a:sysClr>`) to a
/// real RGB value. `slot_name` is used only to pick the fallback value
/// (see below).
///
/// - `<a:srgbClr val="RRGGBB"/>`: parses `val` directly as 6-digit hex.
/// - `<a:sysClr val="..." lastClr="RRGGBB"/>`: `val` (a named system
///   color like `windowText`/`window`) has no OS-independent way to
///   resolve, so it's ignored in favor of `lastClr` (the cached value
///   Excel writes on save —
///   [Issue #76 comment](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352388486);
///   the pragmatic compromise other implementations, including Apache
///   POI, take too).
/// - If `lastClr` itself is missing, or not valid 6-digit hex: falls
///   back to `#FFFFFF` when `slot_name` is `lt1`/`lt2`, or `#000000`
///   otherwise (`dk1`/`dk2`/`accent*`/`hlink`/`folHlink`) — not an error
///   (see Error Handling Policy).
fn resolve_slot_color(slot_name: &str, event: &Event<'_>, path: &str) -> Result<Rgb, Error> {
    let _ = (slot_name, event, path);
    unimplemented!()
}
```

## Dependencies

- Depends on: [`parse/mod.rs`](mod.en.md) (`create_secure_reader`, `convert_xml_error`, `optional_attr`), [`model/color.rs`](../model/color.en.md) (`Rgb`, `ThemePalette`), [`error.rs`](../error.en.md)
- Depended on by: `pipeline.rs` (expected to call this only when a `theme{N}.xml` part exists *and* `StyleSheet` contains at least one `ColorRef::Theme` — see [pipeline.md Open Question 6](../pipeline.en.md)), [`resolve/color.rs`](../resolve/color.en.md) (reads the built `ThemePalette` to resolve `ColorRef::Theme` — does not depend on this file directly; the same as [`model/style.md`](../model/style.en.md)'s `parse/`/`resolve/` split, the two are connected only indirectly through `model/color.rs`'s types)

## Error Handling Policy

- If `<clrScheme>`'s structure itself is broken (XML syntax error), convert it through [`convert_xml_error`](mod.en.md) to `Error::XmlParse` or `Error::ZipBombDetected`
- **If any of the 12 slots is never found, return `Error::MissingRequiredElement`** — a deliberate departure from the graceful-degradation policy `parse/styles.rs` takes for a missing/inconsistent `numFmtId`. A missing `numFmtId` is a legitimately valid state under the spec (degrading to `None` is fine); `ThemePalette`, however, is a fixed 12-element array, and a design that returns a partially-built `ThemePalette` breaks [`model/color.rs`](../model/color.en.md)'s type contract outright. ECMA-376 guarantees `<clrScheme>` has all 12 elements by spec, so an omission is treated not as "ambiguity to tolerate while reading" but as "a corrupted file"
- **An individual slot's color representation (missing `lastClr` on `sysClr`, invalid hex) is never an error — it degrades to the slot-name-dependent fixed fallback described above** — see `resolve_slot_color`'s doc comment. This is a case where "the element exists but its value's interpretation is ambiguous," the same tier of graceful degradation `numFmtId` already applies to individual-value interpretation (distinct from an element being missing outright)

## Test Policy

- Confirm that the actual values from the real fixture (`tests/fixtures/complex/styled_fill_color.xlsx`)'s `theme1.xml`, already PoC-verified (`dk1=000000, lt1=FFFFFF, dk2=1F497D, lt2=EEECE1, accent1=4F81BD, accent2=C0504D, ..., hlink=0000FF, folHlink=800080`), land in the swapped index order (`palette.0[0] == lt1's value`, `palette.0[1] == dk1's value`) — promoting the PoC from [Issue #76 comment](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352366260) into a unit test
- Confirm correct resolution for input where `<clrScheme>`'s children appear in the spec-valid XML declaration order (`dk1,lt1,dk2,lt2,...`)
- **Confirm that `Error::MissingRequiredElement` is returned when any of `<clrScheme>`'s 12 elements (e.g. `accent3`) is missing** — regression test for the fail-closed policy on structural omission
- Confirm `<a:srgbClr val="4F81BD"/>` resolves to `Rgb { r: 0x4F, g: 0x81, b: 0xBD }`
- Confirm `<a:sysClr val="windowText" lastClr="000000"/>` resolves to `lastClr`'s value (`#000000`), ignoring `val`'s value (`windowText`)
- **Confirm that `<a:sysClr val="windowText"/>` without a `lastClr` attribute falls back to `#000000` on `dk1`/`dk2` slots and `#FFFFFF` on `lt1`/`lt2` slots** — a path that never fires on real fixtures, so tested explicitly against synthetic XML ([Issue #76 comment](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352422163) flagged this as a branch coverage doesn't reach naturally)
- Confirm that an invalid hex `lastClr` (e.g. `lastClr="ZZZZZZ"`) degrades to the same fallback value without panicking
- Confirm correct resolution for a `<clrScheme>` declared under a namespace prefix other than `a:` (or no prefix at all), via local-name matching (see Open Question 1)

## Open Questions

1. **Namespace prefix handling**: unlike [parse/mod.md Open Question 4](mod.en.md)'s decision for `r:id` and friends ("skip URI-based resolution via `quick_xml::NsReader`; simplify to prefix-inclusive string prefix matching"), elements under `<clrScheme>` are matched here by `local_name()` (prefix-agnostic local-name matching) — the `drawingml` namespace prefix isn't as practically fixed as `r:id`'s `r` (`a:` is conventional but not spec-mandated), and the element names themselves (`dk1`/`lt1`/...) are a vocabulary unique enough within this schema not to collide. Matching by local name is the safer default over prefix matching here. Whether this judgment still holds at implementation time, against real files with actual prefix variance, remains to be re-verified.
2. **How to resolve `theme{N}.xml`'s actual on-disk path**: the current draft doesn't assume a fixed path like `xl/theme/theme1.xml` (`path` comes from the caller), but how `pipeline.rs` actually resolves it (proper OPC compliance would go through relationship resolution from `xl/_rels/workbook.xml.rels`, though a simplification that reads a fixed `xl/theme/theme1.xml` path directly is also possible — [pipeline.md Open Question 3](../pipeline.en.md) already documents `workbook.xml` itself moving away from exactly this kind of simplification toward relationship resolution) is carried forward as [pipeline.md Open Question 6](../pipeline.en.md).
