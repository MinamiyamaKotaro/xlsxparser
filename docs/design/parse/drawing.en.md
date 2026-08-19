# `parse/drawing.rs` Design Doc

*[日本語](drawing.md)*

Design doc for `src/parse/drawing.rs`. Implements the pure-XML-parsing half of Issue [#65](https://github.com/MinamiyamaKotaro/xlsxparser/issues/65) ("image anchor position / link target cannot be retrieved"): parsing `xl/drawings/drawingN.xml`'s `xdr:twoCellAnchor`/`xdr:oneCellAnchor` elements into `PendingImage` values. Resolving the `r:embed`/hyperlink `r:id` each carries — and locating `drawingN.xml` itself via the worksheet's own `_rels` — is `pipeline.rs`'s job (see [pipeline.md](../pipeline.en.md)'s Phase 3.5), following the same division of labor [relationships.md](relationships.en.md) already established between "parse the routing data" and "interpret/resolve it."

## Responsibility / Scope

- Parses `xl/drawings/drawingN.xml`'s `xdr:twoCellAnchor`/`xdr:oneCellAnchor` elements, each anchoring a `<xdr:pic>` (embedded picture) to a cell position
- For each anchor, extracts:
  - the `xdr:from`/`xdr:to` markers (`TwoCell`) or `xdr:from`/`xdr:ext` (`OneCell`) — cell coordinate plus EMU-unit offset, converted from DrawingML's 0-based `xdr:col`/`xdr:row` to this crate's 1-based `CellRef`
  - the `<xdr:pic>`'s `r:embed` (the embedded media relationship ID) and, if present, `a:hlinkClick`'s `r:id` (the image's own hyperlink relationship ID) — captured as raw strings, not yet resolved to a target path
- Skips (returns nothing for) an anchor with no `<xdr:pic>` inside it — a plain shape or chart anchor, which is out of this Issue's scope
- **Not responsible for**: resolving `embed_r_id`/`hyperlink_r_id` against `drawingN.xml.rels` (`pipeline.rs`'s job — this module never opens a second ZIP entry or does any I/O beyond the single reader it's handed), locating which `drawingN.xml` belongs to which worksheet (`pipeline.rs`, via the worksheet's own `_rels` and the `<drawing r:id="...">` element `parse/worksheet.rs` now also collects), reading the embedded image's own bytes (out of scope for the whole Issue — see the Issue body's rationale: a diff-oriented tool has no use for pixel data, and reading it would scale memory use with image count rather than cell count)

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

`AnchorMarker`/`ImageExtent`/`ImageAnchor`/`Image` themselves live in [`model/sheet.rs`](../model/sheet.en.md) alongside `MergedRegion`/`ColWidthRange` — this module only produces the pieces `model::Image` doesn't yet have (the raw, unresolved relationship IDs), matching how `parse/worksheet.rs` produces `PendingSharedString`/`PendingStyle` rather than a resolved `Cell` directly.

### The 0-based-to-1-based conversion

DrawingML's `xdr:col`/`xdr:row` are 0-based (ECMA-376 Part 1's `ST_ColumnRow` — ancestor of `CT_Marker`), unlike this crate's `CellRef`, which is 1-based to match A1 notation (see [`model/cell.md`](../model/cell.en.md)). `zero_based_to_cell_ref` adds 1 to each before constructing a `CellRef`, and rejects — as `Error::InvalidCellRef` — any value that would overflow `u32` or exceed `CellRef::MAX_ROW`/`MAX_COL` once converted. This mirrors `CellRef::from_a1`'s own bounds check and exists for the same reason (security review `docs/security/code-review.md` Finding 2): an attacker-controlled coordinate from the XML must never reach the model unvalidated, regardless of which code path produced it.

## Dependencies

- Depends on: [`parse/mod.rs`](mod.en.md) (`create_secure_reader`, `read_event`, `read_leaf_text`, `required_attr`, `optional_attr`), [`model/sheet.rs`](../model/sheet.en.md) (`AnchorMarker`, `ImageAnchor`, `ImageExtent`), [`model/cell.rs`](../model/cell.en.md) (`CellRef`), [`error.rs`](../error.en.md)
- `read_leaf_text` was promoted from a `parse/worksheet.rs`-private helper to a shared `parse/mod.rs` function specifically to serve this module too — both modules read plain numeric/text leaf elements (`<v>`, `<xdr:col>`, etc.) with the same "no nested elements expected" shape, distinct from `concat_rich_text`'s richer `<r><t>` run structure
- Depended on by: `pipeline.rs`'s Phase 3.5 (see [pipeline.md](../pipeline.en.md)), which resolves `PendingImage`'s relationship IDs against `drawingN.xml.rels` and builds the final `Vec<model::Image>`

## Error Handling Policy

- A required element (`xdr:from`/`xdr:to` on a `TwoCell` anchor, `xdr:from`/`xdr:ext` on a `OneCell` one, `r:embed` on a `<xdr:pic>`'s `<a:blip>`) missing from an anchor that does have a `<xdr:pic>` is `Error::MissingRequiredElement` — fail-fast, following the same policy `parse/worksheet.rs` applies to a `<c>` missing its `r` attribute
- An anchor with no `<xdr:pic>` at all short-circuits before any of the above checks run and is simply omitted from the result — not an error, since a plain shape/chart anchor legitimately carries none of this
- Malformed numeric content in a leaf element (`xdr:col`/`xdr:colOff`/`xdr:row`/`xdr:rowOff`, or `xdr:ext`'s `cx`/`cy` attributes) is `Error::InvalidPackage`, matching `parse/worksheet.rs::parse_u32_attr`/`parse_f64_attr`'s convention for the same class of failure (a well-formed element carrying content that doesn't parse as its expected type)
- A coordinate that overflows or exceeds `CellRef::MAX_ROW`/`MAX_COL` once converted from 0-based to 1-based is `Error::InvalidCellRef` (see Key Types above)
- Structurally invalid XML converts to `Error::XmlParse`/`Error::ZipBombDetected`/`Error::DoctypeRejected` via the same `create_secure_reader`/`read_event` gateway every `parse/` module uses

## Testing Strategy

- A `twoCellAnchor` with both `<a:blip r:embed>` and `<a:hlinkClick r:id>` present parses into a `PendingImage` with both IDs captured and the anchor's `from`/`to` markers correctly converted to 1-based `CellRef`s with their EMU offsets preserved
- A `oneCellAnchor` with `<xdr:ext>` and no hyperlink parses into a `PendingImage` with `hyperlink_r_id: None` and the `ext`'s `cx`/`cy` preserved
- An anchor with no `<xdr:pic>` (e.g. a bare `<xdr:sp>`) is skipped — produces no `PendingImage`, not an error, even when its `from`/`to` are otherwise well-formed
- A `drawingN.xml` with multiple anchors produces one `PendingImage` per picture anchor, in document order
- A `<xdr:pic>` missing `<a:blip r:embed>` is `Error::MissingRequiredElement { name: "r:embed", .. }`
- An `xdr:row`/`xdr:col` value that overflows `u32` or exceeds `CellRef::MAX_ROW`/`MAX_COL` once converted to 1-based is `Error::InvalidCellRef`
- A malformed `xdr:ext` attribute (non-numeric `cx`/`cy`) is `Error::InvalidPackage`
- An empty `<xdr:wsDr>` (no anchors at all) produces an empty `Vec`

## Open Questions

1. **Shapes, charts, and other non-picture drawing objects**: currently silently skipped (no `PendingImage` produced). If a future need arises to surface these in the output model too (e.g. as a generic "shape" anchor separate from `Image`), this module's per-anchor loop would need a second return channel — not designed here, since Issue #65's stated scope is pictures only.
2. **`editAs` and other anchor-behavior attributes**: `xdr:twoCellAnchor`'s `editAs` attribute (`twoCell`/`oneCell`/`absolute` — how the shape behaves when the underlying cells resize) is not captured. It affects Excel's *live* resize behavior, not the anchor's *current* position/size, which is what a diff-oriented consumer of this library's output cares about — but this is worth revisiting if a future use case needs to distinguish "the image moved" from "the cells around it resized and the image followed."
3. ~~Whether `parse/relationships.rs` needs to support media-embedding rels~~ → **Resolved**: [relationships.md Open Question 1](relationships.en.md) left this undecided; Issue #65 answers it — `parse/relationships.rs`'s existing generic `_rels` parser (already tested against `../media/image1.png`-style relative paths) is reused as-is for both `xl/worksheets/_rels/sheetN.xml.rels` (locating `drawingN.xml`) and `xl/drawings/_rels/drawingN.xml.rels` (locating the embedded media/hyperlink targets), with no changes to that module.
