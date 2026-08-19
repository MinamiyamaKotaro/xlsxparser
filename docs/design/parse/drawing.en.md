# `parse/drawing.rs` Design Doc

*[日本語](drawing.md)*

Design doc for `src/parse/drawing.rs`. Implements the pure-XML-parsing half of Issue [#65](https://github.com/MinamiyamaKotaro/xlsxparser/issues/65) ("image anchor position / link target cannot be retrieved") and Issue [#67](https://github.com/MinamiyamaKotaro/xlsxparser/issues/67) ("grouped images (`<xdr:grpSp>`) are silently mishandled"): parsing `xl/drawings/drawingN.xml`'s `xdr:twoCellAnchor`/`xdr:oneCellAnchor` elements — including every `<xdr:pic>` nested inside them, however deeply wrapped in `<xdr:grpSp>` group shapes — into `PendingImage` values. Resolving the `r:embed`/hyperlink `r:id` each carries — and locating `drawingN.xml` itself via the worksheet's own `_rels` — is `pipeline.rs`'s job (see [pipeline.md](../pipeline.en.md)'s Phase 3.5), following the same division of labor [relationships.md](relationships.en.md) already established between "parse the routing data" and "interpret/resolve it."

## Responsibility / Scope

- Parses `xl/drawings/drawingN.xml`'s `xdr:twoCellAnchor`/`xdr:oneCellAnchor` elements, each anchoring one or more `<xdr:pic>` (embedded picture) to a cell position — directly, or nested inside one or more `<xdr:grpSp>` group shapes (Issue #67)
- For each anchor, extracts the `xdr:from`/`xdr:to` markers (`TwoCell`) or `xdr:from`/`xdr:ext` (`OneCell`) — cell coordinate plus EMU-unit offset, converted from DrawingML's 0-based `xdr:col`/`xdr:row` to this crate's 1-based `CellRef`
- For each `<xdr:pic>` found (however deeply nested), extracts its `r:embed` (the embedded media relationship ID) and, if present, `a:hlinkClick`'s `r:id` (the image's own hyperlink relationship ID) — captured as raw strings, not yet resolved to a target path. A picture directly under the anchor uses the anchor's own `from`/`to`/`ext` as its position/size, unchanged since Issue #65; a picture nested inside one or more `<xdr:grpSp>` has its position/size *resolved* through the enclosing group(s)' `<a:xfrm>` transforms (see `resolve_grouped_pic` below) — the anchor's `from`/`to`/`ext` alone describe only the group's own outer bounding box, not any individual picture inside it
- Skips (contributes nothing for) an anchor, or a group, whose content is not a picture (a plain shape, chart, connector, or an empty group) — out of this Issue's scope
- **Not responsible for**: resolving `embed_r_id`/`hyperlink_r_id` against `drawingN.xml.rels` (`pipeline.rs`'s job — this module never opens a second ZIP entry or does any I/O beyond the single reader it's handed), locating which `drawingN.xml` belongs to which worksheet (`pipeline.rs`, via the worksheet's own `_rels` and the `<drawing r:id="...">` element `parse/worksheet.rs` now also collects), reading the embedded image's own bytes (out of scope for the whole Issue — see the Issue body's rationale: a diff-oriented tool has no use for pixel data, and reading it would scale memory use with image count rather than cell count), the per-shape/per-group parsing overhead this adds even to non-grouped pictures (tracked separately as Issue [#71](https://github.com/MinamiyamaKotaro/xlsxparser/issues/71))

## Key Types / Functions

```rust
use crate::error::Error;
use crate::model::{AnchorMarker, CellRef, ImageAnchor, ImageExtent};
use crate::parse::{create_secure_reader, optional_attr, read_event, read_leaf_text, required_attr};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::BufRead;

/// One `<xdr:pic>` inside a `twoCellAnchor`/`oneCellAnchor`, before its
/// relationship IDs have been resolved to actual target paths. `pipeline.rs`
/// resolves `embed_r_id`/`hyperlink_r_id` against `drawingN.xml.rels` and
/// turns this into `model::Image`.
pub(crate) struct PendingImage {
    pub anchor: ImageAnchor,
    pub embed_r_id: String,
    pub hyperlink_r_id: Option<String>,
}

/// Parses one `drawingN.xml` into every `<xdr:pic>` it anchors.
pub(crate) fn parse_drawing(reader: impl BufRead, path: &str) -> Result<Vec<PendingImage>, Error> {
    // For each <xdr:twoCellAnchor>/<xdr:oneCellAnchor>, parse its
    // <xdr:from>/<xdr:to>/<xdr:ext> markers and, if a <xdr:pic> is present,
    // its <a:blip r:embed>/<a:hlinkClick r:id>. An anchor with no <xdr:pic>
    // is skipped (Ok(None) internally, simply omitted from the result).
    ..
}
```

`AnchorMarker`/`ImageExtent`/`ImageAnchor`/`Image` themselves live in [`model/sheet.rs`](../model/sheet.en.md) alongside `MergedRegion`/`ColWidthRange` — this module only produces the pieces `model::Image` doesn't yet have (the raw, unresolved relationship IDs), matching how `parse/worksheet.rs` produces `PendingSharedString`/`PendingStyle` rather than a resolved `Cell` directly. No `model::` changes were needed for grouped-image support (Issue #67): a resolved grouped picture always becomes `ImageAnchor::OneCell { from, ext }` — an explicit cell + offset + size — regardless of whether the *enclosing* anchor was itself `TwoCell` or `OneCell`, since a picture inside a group has no `to` marker of its own; only its resolved position and size exist.

### Grouped images: `GroupContext` and `resolve_grouped_pic` (Issue #67)

```rust
/// A `<xdr:grpSp>`'s own `<xdr:grpSpPr><a:xfrm>`.
#[derive(Debug, Clone, Copy, Default)]
struct GroupContext {
    off_x: i64,
    off_y: i64,
    ext_cx: i64,
    ext_cy: i64,
    ch_off_x: i64,
    ch_off_y: i64,
    ch_ext_cx: i64,
    ch_ext_cy: i64,
}
```

`parse_anchor_body` maintains `group_stack: Vec<GroupContext>`, pushed on `<xdr:grpSp>`'s start and popped on its end — since `<xdr:grpSp>` *can* nest inside itself (unlike `twoCellAnchor`/`oneCellAnchor`), the stack's own length doubles as the nesting-depth tracker; no separate counter is needed, because well-formed XML always closes nested elements in LIFO order.

`resolve_grouped_pic(group_stack, pic_off, pic_ext)` applies each level's linear transform — `child' = off + (child - chOff) * (ext / chExt)` — from innermost to outermost (`group_stack` is iterated in reverse, since it was built outermost-first while parsing), with one exception: **the outermost group's own `off` is excluded** (treated as 0), because it is taken to coincide with the anchor's own `from` point. The resulting delta is added onto `from.col_off`/`from.row_off` (the anchor's own `from` marker, reused as the base cell for every picture the anchor's group tree contains) to produce the final `AnchorMarker`; the size (`ext`) is scaled the same way, level by level.

This "exclude the outermost `off`" rule was independently confirmed against real LibreOffice-generated output (Issue #67 review discussion): the outermost group's `off`/`ext` are literal absolute-canvas EMU coordinates (not, as first suspected, values already relative to `from`) — but since `from`'s own true absolute position and the group's `off` describe, by construction, the exact same physical point (a geometric necessity for the file to render correctly at all), the two cancel out when only the *delta* is needed. No row-height or column-width lookup is ever required, confirmed both by hand-tracing the algorithm against a synthetic 3-level-deep nesting case and by running it against the real LibreOffice sample's actual numeric values (see the `nested_group_resolves_correctly`/`single_level_group_resolves_each_pic_relative_to_from` tests).

### The 0-based-to-1-based conversion

DrawingML's `xdr:col`/`xdr:row` are 0-based (ECMA-376 Part 1's `ST_ColumnRow` — ancestor of `CT_Marker`), unlike this crate's `CellRef`, which is 1-based to match A1 notation (see [`model/cell.md`](../model/cell.en.md)). `zero_based_to_cell_ref` adds 1 to each before constructing a `CellRef`, and rejects — as `Error::InvalidCellRef` — any value that would overflow `u32` or exceed `CellRef::MAX_ROW`/`MAX_COL` once converted. This mirrors `CellRef::from_a1`'s own bounds check and exists for the same reason (security review `docs/security/code-review.md` Finding 2): an attacker-controlled coordinate from the XML must never reach the model unvalidated, regardless of which code path produced it.

## Dependencies

- Depends on: [`parse/mod.rs`](mod.en.md) (`create_secure_reader`, `read_event`, `read_leaf_text`, `required_attr`, `optional_attr`), [`model/sheet.rs`](../model/sheet.en.md) (`AnchorMarker`, `ImageAnchor`, `ImageExtent`), [`model/cell.rs`](../model/cell.en.md) (`CellRef`), [`error.rs`](../error.en.md)
- `read_leaf_text` was promoted from a `parse/worksheet.rs`-private helper to a shared `parse/mod.rs` function specifically to serve this module too — both modules read plain numeric/text leaf elements (`<v>`, `<xdr:col>`, etc.) with the same "no nested elements expected" shape, distinct from `concat_rich_text`'s richer `<r><t>` run structure
- Depended on by: `pipeline.rs`'s Phase 3.5 (see [pipeline.md](../pipeline.en.md)), which resolves `PendingImage`'s relationship IDs against `drawingN.xml.rels` and builds the final `Vec<model::Image>`

### Local-name collision between `<xdr:ext>` and `<a:ext>` (Issue #65 follow-up)

`parse_anchor_body`'s flat, prefix-agnostic event scan (see Dependencies/parse/mod.md's namespace policy) means `<xdr:ext>` (a `OneCell` anchor's own displayed size, a direct child of `oneCellAnchor`) and `<a:ext>` (inside a `<xdr:pic>`'s own `<xdr:spPr><a:xfrm>`, describing the shape's internal geometry) are indistinguishable by local name alone — both are just "ext" once the prefix is stripped. Real writers (confirmed against actual LibreOffice output, not just a hypothetical) emit `<xdr:spPr><a:xfrm><a:ext>` even on a plain, non-grouped `<xdr:pic>`, and it appears *after* the anchor's own `<xdr:ext>` in document order (`from, ext, pic` per the `CT_OneCellAnchor` schema) — so without a guard, the pic's internal, generally-unrelated size would silently overwrite the anchor's actually-displayed size. Since `<xdr:ext>` is exactly the value a diff-oriented consumer cares about (what's shown on the sheet), this was a real correctness bug, not a cosmetic one.

Fixed via a plain `bool` (`in_pic`, not a depth counter — `<xdr:pic>` never nests inside itself) gating the `<xdr:ext>` match to only fire while `!in_pic`. Grouped-image support (Issue #67) generalized this into a three-way dispatch (group-level / pic-level / anchor-level) rather than a two-way one, since a `<xdr:pic>`'s own `off`/`ext` genuinely does need to be read now — but only when that picture is inside a group (`in_sp_pr` is itself gated on `in_pic && !group_stack.is_empty()`); a non-grouped picture's `spPr` is never entered at all, so its `<a:ext>` still falls through to the `!in_pic` anchor-level branch and is correctly ignored, exactly as before.

A second, related scoping issue surfaced during Issue #67's implementation: `hlinkClick` capture must be gated on `in_pic` too. A hyperlink on the *group itself* (`<xdr:nvGrpSpPr><xdr:cNvPr><a:hlinkClick>`) appears earlier in document order than any picture inside it; without the guard it would bleed into the first picture found. `embed_r_id`/`hyperlink_r_id`/`pic_off`/`pic_ext` are also all reset at each `</xdr:pic>`, so nothing leaks from one grouped picture to the next sibling either.

## Error Handling Policy

- A required element (`xdr:from`/`xdr:to` on a `TwoCell` anchor, `xdr:from`/`xdr:ext` on a `OneCell` one, `r:embed` on a `<xdr:pic>`'s `<a:blip>`) missing from an anchor that does have a `<xdr:pic>` is `Error::MissingRequiredElement` — fail-fast, following the same policy `parse/worksheet.rs` applies to a `<c>` missing its `r` attribute
- An anchor with no `<xdr:pic>` at all short-circuits before any of the above checks run and is simply omitted from the result — not an error, since a plain shape/chart anchor legitimately carries none of this
- Malformed numeric content in a leaf element (`xdr:col`/`xdr:colOff`/`xdr:row`/`xdr:rowOff`, or `xdr:ext`'s `cx`/`cy` attributes) is `Error::InvalidPackage`, matching `parse/worksheet.rs::parse_u32_attr`/`parse_f64_attr`'s convention for the same class of failure (a well-formed element carrying content that doesn't parse as its expected type)
- A coordinate that overflows or exceeds `CellRef::MAX_ROW`/`MAX_COL` once converted from 0-based to 1-based is `Error::InvalidCellRef` (see Key Types above)
- Structurally invalid XML converts to `Error::XmlParse`/`Error::ZipBombDetected`/`Error::DoctypeRejected` via the same `create_secure_reader`/`read_event` gateway every `parse/` module uses
- A `<xdr:grpSp>` whose `chExt` is zero on either axis (an undefined scale factor — `resolve_grouped_pic`'s `ext / chExt`) is `Error::InvalidPackage`, fail-fast — matching this file's general convention of rejecting a well-formed-but-nonsensical numeric value rather than silently producing `NaN`/`Infinity` (Issue #67)
- `<xdr:grpSp>` nesting beyond `MAX_GROUP_NESTING_DEPTH` (64) is `Error::TooManyNestedGroups`, checked at the point a nested group's start tag would push `group_stack` past the limit — before any further content of that group is read (security review Finding 1, Issue #71 follow-up: see below)
- A resolved group-transform coordinate (`resolve_grouped_pic`'s final `x`/`y`/`cx`/`cy`) that is non-finite (`NaN`/`Infinity`, reachable via a crafted extreme `ext`/`chExt` ratio compounded across nesting levels) or exceeds a defensive plausibility ceiling (`MAX_PLAUSIBLE_EMU`, 10^12 EMU — several orders of magnitude beyond Excel's real maximum sheet extent) is `Error::InvalidPackage`, fail-fast, checked once after the transform loop completes (security review Finding 2, Issue #71 follow-up)

## Testing Strategy

- A `twoCellAnchor` with both `<a:blip r:embed>` and `<a:hlinkClick r:id>` present parses into a `PendingImage` with both IDs captured and the anchor's `from`/`to` markers correctly converted to 1-based `CellRef`s with their EMU offsets preserved
- A `oneCellAnchor` with `<xdr:ext>` and no hyperlink parses into a `PendingImage` with `hyperlink_r_id: None` and the `ext`'s `cx`/`cy` preserved
- An anchor with no `<xdr:pic>` (e.g. a bare `<xdr:sp>`) is skipped — produces no `PendingImage`, not an error, even when its `from`/`to` are otherwise well-formed
- A `drawingN.xml` with multiple anchors produces one `PendingImage` per picture anchor, in document order
- A `<xdr:pic>` missing `<a:blip r:embed>` is `Error::MissingRequiredElement { name: "r:embed", .. }`
- An `xdr:row`/`xdr:col` value that overflows `u32` or exceeds `CellRef::MAX_ROW`/`MAX_COL` once converted to 1-based is `Error::InvalidCellRef`
- A malformed `xdr:ext` attribute (non-numeric `cx`/`cy`) is `Error::InvalidPackage`
- A `oneCellAnchor` whose `<xdr:pic>` carries its own `<xdr:spPr><a:xfrm><a:ext>` with a *different* `cx`/`cy` than the anchor's own `<xdr:ext>` resolves to the anchor's value, not the pic's internal one (Issue #65 follow-up — see the local-name collision note above)
- An empty `<xdr:wsDr>` (no anchors at all) produces an empty `Vec`
- A single-level `<xdr:grpSp>` with two pictures resolves each to its own `ImageAnchor::OneCell`, sharing the anchor's `from.cell` but with distinct `col_off`/`row_off` deltas and sizes — verified against real numeric values captured from actual LibreOffice output (Issue #67)
- A 3-level-deep nested group resolves a single picture correctly — verified by hand-tracing the transform against a synthetic case matching the pure-math PoC's own validated Case 3
- A hyperlink on the group itself (`<xdr:nvGrpSpPr>`'s `<a:hlinkClick>`) does not leak into the first picture's `hyperlink_r_id`
- A hyperlink on one grouped picture does not leak into the next sibling picture that has none
- A `<xdr:grpSp>` with `chExt` zero on either axis is `Error::InvalidPackage`
- A group containing only non-picture shapes (no `<xdr:pic>` anywhere inside it) contributes no images
- `<xdr:grpSp>` nesting exactly at `MAX_GROUP_NESTING_DEPTH` is accepted; one level over is `Error::TooManyNestedGroups` (security review Finding 1)
- A crafted `ext`/`chExt` ratio compounded across a small number of nesting levels, driving the resolved coordinate to a non-finite or implausibly large value, is `Error::InvalidPackage` (security review Finding 2)

## Open Questions

1. **Shapes, charts, and other non-picture drawing objects**: currently silently skipped (no `PendingImage` produced). If a future need arises to surface these in the output model too (e.g. as a generic "shape" anchor separate from `Image`), this module's per-anchor loop would need a second return channel — not designed here, since Issue #65's stated scope is pictures only.
2. **`editAs` and other anchor-behavior attributes**: `xdr:twoCellAnchor`'s `editAs` attribute (`twoCell`/`oneCell`/`absolute` — how the shape behaves when the underlying cells resize) is not captured. It affects Excel's *live* resize behavior, not the anchor's *current* position/size, which is what a diff-oriented consumer of this library's output cares about — but this is worth revisiting if a future use case needs to distinguish "the image moved" from "the cells around it resized and the image followed."
3. ~~Whether `parse/relationships.rs` needs to support media-embedding rels~~ → **Resolved**: [relationships.md Open Question 1](relationships.en.md) left this undecided; Issue #65 answers it — `parse/relationships.rs`'s existing generic `_rels` parser (already tested against `../media/image1.png`-style relative paths) is reused as-is for both `xl/worksheets/_rels/sheetN.xml.rels` (locating `drawingN.xml`) and `xl/drawings/_rels/drawingN.xml.rels` (locating the embedded media/hyperlink targets), with no changes to that module.
4. ~~Whether `<xdr:grpSp>` (grouped images) is supported, and if so how~~ → **Resolved**: Issue #67 — see the "Grouped images" section above. Group children always resolve to `ImageAnchor::OneCell`.
5. **Per-shape parsing overhead added by Issue #67, even for non-grouped pictures**: tracked as its own issue, [#71](https://github.com/MinamiyamaKotaro/xlsxparser/issues/71) — PoC benchmarking (against real LibreOffice output) measured roughly +20% per-shape cost from the larger `match` in `parse_anchor_body`, attributable to the added arms being *checked* on every XML event even when their guarded bodies never run for a non-grouped picture. Absolute cost stays in the hundreds-of-nanoseconds range (negligible next to a whole workbook's ~190µs parse time in the measured fixture), but is worth revisiting if a real-world file with very many images makes it measurable.
6. **`Image::hyperlink` for a group-level hyperlink**: a hyperlink attached to the *group itself* (rather than to an individual picture inside it) is intentionally not surfaced anywhere in the output model — only per-picture hyperlinks are (Issue #65's original scope). Revisit if a real-world file is found to rely on this.
7. ~~Whether `<xdr:grpSp>` nesting depth needs a defensive bound~~ → **Resolved**: it does. `resolve_grouped_pic` costs O(current nesting depth) per `<xdr:pic>` resolved, so a drawing part with `D` levels of nesting and `N` sibling pictures at the innermost level costs O(N × D) while costing only O(N + D) bytes to construct — the Zip Bomb byte-size cap alone does not bound `D`, the same shape `docs/security/old/code-review.en.md` Finding 1 found for merge-cell count. Measured (`docs/security/design-review.en.md` Finding 1): a 22.6 MB drawing part (D=N=50,000) took 10.9s of synchronous CPU time before the fix. Added `MAX_GROUP_NESTING_DEPTH = 64` (`parse::drawing`), checked at each `<xdr:grpSp>` start tag — the same defensive-cap pattern `resolve::merge::MAX_MERGE_REGIONS`/`resolve::column_width::MAX_COLUMN_WIDTH_RANGES` already establish. Re-measured post-fix: the identical attack shape now rejects in under 1ms. A related, separate numeric-magnitude bound (`MAX_PLAUSIBLE_EMU`) was added at the same time — see Error Handling Policy above and `docs/security/design-review.en.md`/`code-review.en.md` Findings 1–2 for full detail.
