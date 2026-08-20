# `parse/worksheet.rs` Design Doc

*[日本語](worksheet.md)*

Design doc for `src/parse/worksheet.rs`. Per [architecture.md](../architecture.en.md), this file *is* Phase 3: "SAX-style streaming parse of `sheetX.xml` (row-level disposal is completed here)." To satisfy requirements chapter 3's core functionality (sparse-matrix memory optimization, transparent access to merged cells), it streams cells into [`model::Sheet::insert_cell`](../model/sheet.en.md) while shaping shared-string, style, and merge-range information that needs deferred resolution into the form [`resolve/`](../resolve/mod.en.md) (Phase 4) consumes. This file finalizes the contract that [resolve/mod.md](../resolve/mod.en.md), [resolve/shared_strings.md](../resolve/shared_strings.en.md), [resolve/style.md](../resolve/style.en.md), and [resolve/merge.md](../resolve/merge.en.md) had all assumed while noting "`parse/worksheet.rs` is not yet designed."

## Responsibility / Scope

- Streams `xl/worksheets/sheetX.xml`'s `<sheetData>` row (`<row>`) by row, building a [`Cell`](../model/cell.en.md) for each `<c>` and inserting it via `Sheet::insert_cell`
- Once one row's worth of data is fully processed (every `<c>` belonging to that row has been read and reflected via `insert_cell`), discards the parser's internal state for that row (attributes, text buffers, etc.) before moving on to the next row (implements the requirements' Phase 3 requirement; per architecture.md, "row-level XML node disposal is an internal implementation detail of `parse/worksheet.rs`; `pipeline.rs` does not control it")
- When a `t="s"` (shared-string index reference) cell is detected, inserts a `Cell` with `value: None` via `insert_cell` while simultaneously recording a corresponding `PendingSharedString` (defined by this file; consumed by [`resolve/shared_strings.rs`](../resolve/shared_strings.en.md))
- When a cell carries an `s` (`cellXfs` index — style ID reference) attribute, records a corresponding `PendingStyle` (defined by this file; consumed by [`resolve/style.rs`](../resolve/style.en.md))
- `t="str"` (formula-computed string result) and `t="inlineStr"` (inline string) cells need no deferred resolution, so they are resolved directly to `CellValue::Text` during the stream and inserted via `insert_cell` (the division of labor already assumed by [resolve/shared_strings.md Responsibility/Scope](../resolve/shared_strings.en.md))
- Collects `<mergeCells><mergeCell ref="A1:C3"/>...</mergeCells>`, which appears after the stream completes (after `</sheetData>`), converting each `ref` into `start`/`end` via [`CellRef::from_a1`](../model/cell.en.md) and gathering them into `Vec<MergedRegion>` (the stage that feeds into [`resolve/merge.rs`](../resolve/merge.en.md)'s validation and registration)
- Collects `<cols><col min=".." max=".." width=".."/>...</cols>`, which appears *before* `<sheetData>` (per the OOXML schema's fixed element order), into `Vec<ColWidthRange>` — one entry per `<col>` that actually carries a `width` attribute (a `<col>` that only sets e.g. `hidden`/`bestFit` is skipped, since this file tracks nothing else about columns yet). Also collects `<sheetFormatPr defaultColWidth="..">` if present. Both feed into [`resolve/column_width.rs`](../resolve/column_width.en.md)'s validation and registration (Issue #39), the same two-phase split `<mergeCells>` already established
- Collects `<hyperlinks><hyperlink ref="A1" r:id="rId1" location=".." tooltip=".."/>...</hyperlinks>` (Issue #95), which appears after `<sheetData>` like `<mergeCells>`, into `Vec<PendingHyperlink>` — `ref` is parsed as either a single coordinate or a `start:end` range (both `CellRef`s kept; no ordering validation here, same split as `<mergeCells>`), `r:id`/`location`/`tooltip` kept as-is. Feeds `pipeline.rs`'s Phase 3.5 (`r:id` → raw Target string, needs ZIP I/O) and then [`resolve/hyperlink.rs`](../resolve/hyperlink.en.md)'s validation and registration
- **Not responsible for**: actually resolving shared-string indices or style IDs ([`resolve/shared_strings.rs`](../resolve/shared_strings.en.md) / [`resolve/style.rs`](../resolve/style.en.md)), validating merge ranges, hyperlink ranges, or column-width ranges or registering any of them into `Sheet` ([`resolve/merge.rs`](../resolve/merge.en.md) / [`resolve/hyperlink.rs`](../resolve/hyperlink.en.md) / [`resolve/column_width.rs`](../resolve/column_width.en.md) — this file only collects the raw lists; it never calls `insert_merge`/`finalize_hyperlinks`/`set_col_widths`), resolving a hyperlink's `r:id` against `_rels` (`pipeline.rs`, needs ZIP I/O), parsing or retaining formulas (`<f>` elements — see Open Question 2)

## Key Types / Functions (draft)

```rust
use crate::error::Error;
use crate::model::cell::{Cell, CellRef, CellValue};
use crate::model::sheet::{MergedRegion, Sheet};
use crate::model::style::StyleId;
use crate::parse::{concat_rich_text, convert_xml_error, create_secure_reader, required_attr};
use std::io::BufRead;
use std::sync::Arc;

/// The pending entry Phase 3 records when it detects a `t="s"` cell.
/// `model::CellValue` only ever admits an already-resolved `Text(Arc<str>)`
/// and has no variant that holds a raw index (see
/// [model/cell.md](../model/cell.en.md)), so at parse time the cell itself
/// is inserted into `Sheet` with `value: None` (other fields such as style
/// are set as usual), and the index is kept outside the sheet in this
/// struct instead. [`resolve/shared_strings.rs`](../resolve/shared_strings.en.md)
/// consumes this to resolve the actual string (per the [PR #9
/// review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204),
/// this type's definition was relocated here, since it is Phase 3's own
/// output data — see Dependencies for the rationale).
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingSharedString {
    pub cell_ref: CellRef,
    pub index: usize,
}

/// The pending entry Phase 3 records when it detects a cell carrying an `s`
/// (style index) attribute. [`resolve/style.rs`](../resolve/style.en.md)
/// consumes this to apply the `ResolvedStyle` (relocated for the same
/// reason as `PendingSharedString` — see Dependencies).
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingStyle {
    pub cell_ref: CellRef,
    pub style_id: StyleId,
}

/// `parse_worksheet`'s output. `sheet` itself is mutated directly through
/// the `&mut` argument, so this only returns the three remaining pieces of
/// unresolved data Phase 4 ([resolve/mod.rs](../resolve/mod.en.md)'s
/// `resolve_sheet`) needs as-is.
pub(crate) struct WorksheetParseOutput {
    pub pending_shared_strings: Vec<PendingSharedString>,
    pub pending_styles: Vec<PendingStyle>,
    pub merge_regions: Vec<MergedRegion>,
}

/// Phase 3's entry function. `sheet` is received already constructed by
/// `pipeline.rs` with `name`/`visibility` set from [`parse/workbook.rs`](workbook.en.md)'s
/// result; cells are streamed into it.
///
/// Calling contract (the counterpart, owned by this file, to [resolve/mod.md's
/// calling precondition](../resolve/mod.en.md)):
/// - When a `t="s"` cell is detected, `insert_cell`-ing a `Cell` with
///   `value: None` and recording the corresponding `PendingSharedString`
///   must always happen together (resolves [resolve/shared_strings.md Open
///   Question 2](../resolve/shared_strings.en.md)).
/// - When a cell carries an `s` attribute, `insert_cell` and recording the
///   corresponding `PendingStyle` must always happen together.
///   [`resolve/shared_strings.rs`](../resolve/shared_strings.en.md) and
///   [`resolve/style.rs`](../resolve/style.en.md) both `expect()` the result
///   of `Sheet::get_mut` on the assumption that this invariant holds.
///
/// `date1904` is `workbook.xml`'s `<workbookPr date1904="1"/>` flag,
/// otherwise a purely Phase 4 ([`resolve/style.rs`](../resolve/style.en.md))
/// concern — it reaches Phase 3 solely so a `t="d"` time-only cell's
/// placeholder date (no date component exists in the ISO 8601 source text)
/// can agree with what a numeric time-only cell in the same book resolves
/// to (PR #80 review point 2; see `parse_iso8601_datetime`'s doc comment).
///
/// Both `<row>` and `<c>`'s `r` attribute (cell/row reference) are optional
/// per the ECMA-376 spec; when omitted, the position is inferred as "right
/// after the previous row/cell" (Issue #79). This function keeps the
/// current row number and the current column position within that row as
/// loop-local state: a `<row>` start tag settles the row number and resets
/// the column counter; each `<c>` start tag either adopts its own `r` (and
/// updates the current column from it) or, if `r` is absent, advances the
/// current column by one. Either path applies the same
/// `CellRef::MAX_ROW`/`MAX_COL` bound [`model/cell.rs`](../model/cell.en.md)
/// already enforces for an explicit `r` — a run of `r`-omitting `<c>`
/// elements would otherwise never pass through `CellRef::from_a1`'s own
/// check at all, letting it inflate `Sheet::max_col` unbounded (the same
/// attack surface security review `docs/security/code-review.md` Finding 2
/// covers).
pub(crate) fn parse_worksheet(
    reader: impl BufRead,
    path: &str,
    sheet: &mut Sheet,
    date1904: bool,
    max_cells: usize,
) -> Result<WorksheetParseOutput, Error> {
    let mut xml_reader = create_secure_reader(reader);
    let mut pending_shared_strings = Vec::new();
    let mut pending_styles = Vec::new();
    let mut merge_regions = Vec::new();
    // Implementation plan: a state machine transitioning between "outside
    // <sheetData>", "inside <row>", and "inside <c>" (holding t/s
    // attributes, waiting for <v>/<is> text). At the End event for <row>,
    // clear that row's scratch state (any open attributes, text buffers) in
    // preparation for the next <row>'s Start event. quick-xml's own
    // read_event_into(&mut buf) also clears and reuses `buf` on every call,
    // so no multi-row backlog of XML nodes ever accumulates on the heap.
    let _ = (
        &mut xml_reader,
        path,
        sheet,
        &mut pending_shared_strings,
        &mut pending_styles,
        &mut merge_regions,
    );
    unimplemented!()
}

/// Builds a `Cell` from the content of `<v>`/`<is>` based on `<c t="...">`'s
/// `t` attribute (absent implies Number). For `t="s"` and any cell carrying
/// an `s` attribute, returns `value`/`style` as `None` and leaves the
/// caller (`parse_worksheet`) to record the corresponding `Pending*`.
fn build_cell(
    cell_ref: CellRef,
    cell_type: Option<&str>,
    style_id: Option<u32>,
    value_text: Option<&str>,
    inline_string: Option<String>,
    date1904: bool,
) -> Result<Cell, Error> {
    let value = match cell_type {
        None | Some("n") => value_text.map(parse_number).transpose()?.map(CellValue::Number),
        Some("s") => None, // resolved by resolve/shared_strings.rs (recorded separately as a PendingSharedString)
        Some("str") => value_text.map(|s| CellValue::Text(Arc::from(s))),
        Some("inlineStr") => inline_string.map(|s| CellValue::Text(Arc::from(s))),
        Some("b") => value_text.map(|s| s == "1").map(CellValue::Boolean),
        Some("e") => value_text.map(|s| CellValue::Error(s.to_string())),
        // ECMA-376 Part 1's t="d" extension (Issue #58): <v>'s text is an
        // ISO 8601 string rather than a serial number.
        Some("d") => value_text
            .map(|s| parse_iso8601_datetime(s, date1904))
            .transpose()?
            .map(CellValue::DateTime),
        // Unknown `t` value: falls back to keeping the raw text as Text
        // rather than dropping data — see Open Question 3.
        Some(_) => value_text.map(|s| CellValue::Text(Arc::from(s))),
    };
    let _ = (cell_ref, style_id);
    Ok(Cell { value, style: None })
}

fn parse_number(text: &str) -> Result<f64, Error> {
    let _ = text;
    unimplemented!()
}

/// Parses a `t="d"` cell's `<v>` text as ISO 8601 (Issue #58). Handles the
/// three shapes observed in a real file
/// (`tests/fixtures/other/date_iso.xlsx`, from calamine's test corpus) —
/// date-only (`2021-01-01`), date+time (`2021-01-01T10:10:10`), and
/// time-only (`10:10:10`) — plus the following spec-valid variations a
/// writer other than the one that produced that fixture might still emit
/// (PR #80 review point 1):
/// - a trailing UTC/offset designator (`Z`, `+09:00`, `-0500`) is dropped —
///   `DateTimeValue` has no timezone field (mirroring Excel's own date
///   system, which isn't timezone-aware either), so the wall-clock value is
///   kept as-is rather than converted
/// - fractional seconds (`10:10:10.500`) are truncated to whole seconds
/// - seconds may be omitted entirely (`10:10`), defaulting to `:00`
///
/// Anything else malformed (wrong segment count, an out-of-range number)
/// still errors, as before.
///
/// A time-only value has no date component in the source text, so it lands
/// on Excel's own "time of day with no date" convention: serial day 0,
/// which is 1899-12-30 under the default 1900 date system or 1904-01-01
/// under `date1904` — matching how
/// [`resolve/style.rs`](../resolve/style.en.md)'s `serial_to_date_time`
/// already decodes a fractional serial < 1 for numeric (non-ISO) cells in
/// the same book (PR #80 review point 2). This is the reason this
/// otherwise Phase-4-flavored flag reaches Phase 3 at all (see
/// `parse_worksheet`'s doc comment). Phase 3 cannot reference
/// `resolve/style.rs`'s private `EPOCH_OFFSET_1900`/`EPOCH_OFFSET_1904`
/// constants directly (architecture.md design policy 2: `parse/` never
/// depends on `resolve/`), so the same two placeholder dates are
/// hardcoded again here — check both files when either changes.
fn parse_iso8601_datetime(text: &str, date1904: bool) -> Result<DateTimeValue, Error> {
    let _ = (text, date1904);
    unimplemented!()
}
```

## Dependencies

- Depends on: [`parse/mod.rs`](mod.en.md) (`create_secure_reader`, `convert_xml_error`, `required_attr`, `optional_attr`, `concat_rich_text`), [`model/cell.rs`](../model/cell.en.md) (`Cell`, `CellRef`, `CellValue`, `DateTimeValue`. `DateTimeValue` is a new dependency added for `t="d"` support (Issue #58) — a second construction path for the type [Issue #40](https://github.com/MinamiyamaKotaro/xlsxparser/issues/40) introduced, independent of the serial-value path (`resolve/style.rs`)), [`model/sheet.rs`](../model/sheet.en.md) (`Sheet::insert_cell`, `MergedRegion`), [`model/style.rs`](../model/style.en.md) (`StyleId`, used as `PendingStyle`'s field type), [`error.rs`](../error.en.md). Depends on no module under `resolve/`
- Depended on by: `pipeline.rs` (Phase 3 — called once per sheet, passing the return value straight through to [`resolve::resolve_sheet`](../resolve/mod.en.md) plus, separately, `pending_hyperlinks` into its own Phase 3.5/`resolve::hyperlink::resolve` path — see [pipeline.md](../pipeline.en.md). `date1904` — a new argument added for `t="d"` support, Issue #58 / PR #80 review point 2 — is the same value `pipeline.rs` already read in Phase 1 from `parse_workbook_xml`, passed straight through), [`resolve/shared_strings.rs`](../resolve/shared_strings.en.md) (`use`s this file's `PendingSharedString`), [`resolve/style.rs`](../resolve/style.en.md) (same, `PendingStyle`), [`resolve/mod.rs`](../resolve/mod.en.md) (references both types in `resolve_sheet`'s signature), [`resolve/hyperlink.rs`](../resolve/hyperlink.en.md) (indirectly — receives `HyperlinkRange`s `pipeline.rs` builds from this file's `PendingHyperlink`, not `PendingHyperlink` itself)

**Why `PendingSharedString` / `PendingStyle` are defined here**: the original draft defined both types on the consumer side ([`resolve/shared_strings.rs`](../resolve/shared_strings.en.md) / [`resolve/style.rs`](../resolve/style.en.md)), with this file `use`-ing them in reverse — an unnatural "parser layer (lower) → resolve layer (higher)" dependency (acyclic, but against the spirit of architecture.md design policy 2, separating the I/O layer (`container`/`parse`) from domain logic (`resolve`)). Per the [PR #9 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204), both types were relocated to this file (`parse/worksheet.rs`), matching what they actually are: Phase 3's own output data. This makes the dependency direction fully one-directional (a DAG), unifying it with the pattern [`resolve/shared_strings.rs`](../resolve/shared_strings.en.md) already established by depending on `parse::shared_strings::SharedStringTable` (per [resolve/mod.md Dependencies](../resolve/mod.en.md)) — "`resolve/` depends on already-built structured data from `parse/`":

```text
parse::worksheet ─┬─▶ resolve::shared_strings (uses PendingSharedString)
                   ├─▶ resolve::style (uses PendingStyle)
                   └─▶ resolve::mod (references PendingSharedString/PendingStyle in resolve_sheet's signature)
parse::shared_strings ─▶ resolve::shared_strings (uses SharedStringTable)
```

The structure now fully matches the spirit of architecture.md design policy 2: no module under `parse/` `use`s any type from `resolve/` (resolves the former Open Question 1).

## Error Handling Policy

- Structurally invalid XML is converted into `Error::XmlParse` or `Error::ZipBombDetected` via [`convert_xml_error`](mod.en.md)
- `<row>` and `<c>`'s `r` attribute are both optional; when omitted, the position is inferred from the previous row/cell (Issue #79 — resolves former Open Question 4). Returns `Error::InvalidCellRef` when a `<row r="...">` value fails to parse as a number, is `0`, or exceeds `CellRef::MAX_ROW`, and when an inferred column exceeds `CellRef::MAX_COL` — the same convention `CellRef::from_a1` already applies. Also `Error::InvalidCellRef` if a `<c>` omits `r` before any `<row>` has been seen at all. A `<row>` `r` failing to parse as a number is deliberately `Error::InvalidCellRef` rather than the `Error::InvalidPackage` `<col>`/`sheetFormatPr` use for their own malformed numeric attributes — `r` is a coordinate here too, so a malformed value is reported the same way a malformed `<c r="...">` already is (PR #81 review). The error text includes the raw attribute value and `path`
- If `r`'s value is not well-formed A1 notation (`CellRef::from_a1` returns `Err`), that `Error::InvalidCellRef` propagates unchanged
- If `<v>`'s numeric text cannot be parsed as `f64`, this returns `Error::InvalidPackage` (provisional; whether a more specific variant is warranted is left to a future revision of [error.md](../error.en.md))
- If a `t="d"` cell's `<v>` text doesn't match any of the date-only / date+time / time-only shapes, or has an out-of-range numeric component (month 13, hour 24, etc.), returns `Error::InvalidPackage` (same provisional convention as the numeric `<v>` case above. Issue #58). Fractional seconds, a trailing UTC/offset designator, and an omitted seconds field are all tolerated rather than rejected (PR #80 review point 1 — see `parse_iso8601_datetime`'s doc comment)
- If a `<mergeCell ref="...">`'s `ref` value is not in the `"A1:C3"` shape (two `:`-separated coordinates), or either coordinate is not well-formed A1 notation, `Error::InvalidCellRef` propagates. This file does not validate the range's soundness itself (start/end ordering, overlap with other ranges) — it simply appends to `merge_regions` and leaves validation to [`resolve/merge.rs`](../resolve/merge.en.md)
- If a `<col>`'s `width`/`defaultColWidth` cannot be parsed as `f64`, or its `min`/`max` cannot be parsed as `u32`, returns `Error::InvalidPackage` (same provisional convention as the numeric `<v>` case above). This file does not validate range soundness (overlap, count) — that is [`resolve/column_width.rs`](../resolve/column_width.en.md)'s responsibility, same division of labor as `<mergeCells>`
- If a `<hyperlink ref="...">`'s value is neither a single well-formed A1 coordinate nor a `"A1:C3"` range of two, `Error::InvalidCellRef` propagates (Issue #95). This file does not validate range soundness (start/end ordering, overlap with other hyperlink ranges) — same division of labor as `<mergeCells>`, deferred to [`resolve/hyperlink.rs`](../resolve/hyperlink.en.md)
- `<f>` (formula) element content is neither parsed nor retained — it is skipped (outside the requirements' scope; see Open Question 2)
- Once the number of cells actually inserted via `Sheet::insert_cell` (the `max_cells` argument, `SizeLimits::max_cells_per_sheet` — see [container/sanitize.md](../container/sanitize.en.md)) exceeds `max_cells`, returns `Error::TooManyCells` (Issue [#88](https://github.com/MinamiyamaKotaro/xlsxparser/issues/88)). A cell dropped for carrying no value/style/shared-string reference is never counted. Unlike `resolve::merge`/`resolve::column_width`, which check in one batch after collection, this counts and checks incrementally while streaming `<c>` elements — for cells, the memory cost accrues the moment one is inserted, so checking only after collecting everything would already be too late
- **Never panics**: since this file handles untrusted external input, any unexpected structure must always propagate as some `Error` variant

## Testing Strategy

- Verify that a minimal `worksheet.xml` with a single row and multiple cells (a mix of number, shared-string reference, boolean, and error values) inserts the expected cells into `Sheet`
- Verify that detecting a `t="s"` cell both inserts a `Cell` with `value: None` and records a corresponding `PendingSharedString` (`cell_ref`/`index` correct) — a wiring test for the invariant
- Verify that a cell carrying an `s` attribute records a corresponding `PendingStyle` (`cell_ref`/`style_id` correct)
- Verify that a `t="str"` cell bypasses `PendingSharedString` and resolves directly to `CellValue::Text` during the stream
- Verify that `t="inlineStr"` cells (both a simple `<is><t>...</t></is>` and rich-text-run form) resolve correctly to `CellValue::Text`
- Verify that a cell element with no value and no style, or a `<row>` with zero `<c>` children, inserts nothing into `Sheet` (sparse matrix — blank cells are never instantiated, per requirements 3.1)
- Verify that no parser-internal state survives from one `<row>` into the next (a regression test for row-level disposal — e.g. that a normal cell in the row right after one containing a `t="s"` cell is never mistakenly treated as a shared string, catching cross-row contamination)
- Verify that multiple `<mergeCell ref="...">` entries under `<mergeCells>` produce a list of `MergedRegion` with correct `start`/`end` (since this file performs no validation itself, this includes confirming that even a malformed range with reversed start/end still ends up in the returned `Vec` unchanged — validation itself is [resolve/merge.md Testing Strategy](../resolve/merge.en.md)'s responsibility)
- Verify that a `<c>` missing `r` is correctly inferred as the column right after the previous cell in the same row — and that a cell with an explicit `r`, followed by one omitting it, resumes counting from that explicit position rather than restarting at the row's start (Issue #79)
- Verify that a `<row>` missing `r` is correctly inferred as the row right after the previous one (Issue #79)
- Verify that each returns `Error::InvalidCellRef`: a `<c>` omitting `r` before any `<row>` has been seen, a `<row r="...">` that fails to parse as a number, a `<row r="0">`, a `<row>` `r` beyond `CellRef::MAX_ROW`, and an inferred column beyond `CellRef::MAX_COL` (a regression test for the same attack surface security review Finding 2 covers) (Issue #79; the non-numeric case was added per PR #81 review — a regression test against the prior behavior, which surfaced `Error::InvalidPackage` instead)
- Verify that a malformed A1-notation `r` attribute or `mergeCell ref` attribute returns `Error::InvalidCellRef`
- Verify that `<hyperlink ref="A1" r:id=".." location=".." tooltip=".."/>` entries under `<hyperlinks>` produce `PendingHyperlink`s with the right `start`/`end`/`r_id`/`location`/`tooltip` — both for a single-coordinate `ref` (`start == end`) and a range `ref` — and that a malformed `ref` returns `Error::InvalidCellRef` without registering anything (Issue #95)
- Verify that for a cell containing an `<f>` element (a formula cell), the `<f>` content is ignored and only `<v>` (the cached computed value) is used as the `Cell`'s value
- Verify that `t="d"` cells resolve to the correct `CellValue::DateTime` for all three shapes — date-only, date+time, and time-only (including that the date component for a time-only value comes out as Excel's convention, 1899-12-30) — and that a malformed shape (wrong segment count, an out-of-range number) returns `Error::InvalidPackage` (Issue #58; all three shapes were confirmed against the real-world `tests/fixtures/other/date_iso.xlsx`, from calamine's test corpus, but that directory is `.gitignore`d and not part of this repo, so the integration test itself reproduces the same three shapes via a hand-authored fixture)
- Verify that a trailing `Z`/`+09:00`-style UTC or offset designator is dropped, that fractional seconds (`10:10:10.500`) are truncated to whole seconds, and that an omitted seconds field (`10:10`) defaults to `:00` (PR #80 review point 1)
- Verify that resolving a time-only `t="d"` cell in a `date1904 = true` book lands on the placeholder date 1904-01-01 rather than 1899-12-30 (PR #80 review point 2 — a regression test for consistency with `resolve/style.rs`'s numeric time-only cell handling)
- Verify that `<cols>` entries with a `width` attribute are collected into `ColWidthRange`s with correct `min`/`max`/`width`, that a `<col>` without `width` is skipped, and that a single `<col min="1" max="16384" .../>` (the realistic worst case) is collected as exactly one range rather than expanded
- Verify that `<sheetFormatPr defaultColWidth="..">` is collected, and that its absence leaves `default_col_width: None`
- Verify that a malformed `<col>`/`<sheetFormatPr>` numeric attribute returns `Error::InvalidPackage`
- Verify that `Error::TooManyCells` is returned as soon as the number of actually-inserted cells exceeds `max_cells`, and that no further parsing continues (Issue #88) — including that a `<c>` carrying no value, style, or shared-string reference is never inserted and so never counts toward `max_cells`, no matter how many appear (`tests/security.rs` verifies this cheaply with a small `max_cells_per_sheet`; fixture is `tests/fixtures/security.rs`'s `too_many_cells`)

## Implementation Notes

- **State machine shape**: implemented with flat `cur_*` local variables (`cur_ref: Option<CellRef>` doubling as "are we inside a `<c>`?", plus `cur_type`/`cur_style`/`cur_value_text`/`cur_inline`) rather than a dedicated state enum, since `<c>`'s children (`<v>`, `<f>`, `<is>`) never nest into each other. Each is freshly reset the moment a `<c>` start tag is seen and fully consumed (via a shared `flush_cell` helper) by its end tag (or immediately, for a self-closing `<c/>`), which is what actually guarantees no state leaks across cells or rows — not an explicit per-row reset.
- **`build_cell` signature**: dropped the draft's unused `cell_ref`/`style_id` parameters (the draft itself discarded them via `let _ = (cell_ref, style_id);`) — the value/style split is fully handled by the caller (`flush_cell`), not `build_cell` itself.
- **`<v>`/`<f>` text reading**: added a `read_leaf_text` helper (not in the draft) that reads a leaf element's text content — including `Event::GeneralRef` entities via [`parse/mod.rs`](mod.en.md)'s `push_general_ref`, the same helper `concat_rich_text` uses — since quick-xml 0.41 tokenizes entities separately from `Event::Text` (see [parse/mod.md Open Question 1](mod.en.md)).
- **`flush_cell`'s insert decision**: a `<c>` is inserted only if it carries a style (`s` attribute), a value (`<v>`/`<is>` text), or is a `t="s"` reference (which will gain a value once resolved) — matching the sparse-matrix requirement that a fully blank `<c r="A1"/>` never gets instantiated.
- **`flush_cell`'s return value** (Issue #88): changed to return whether it actually inserted a cell, as a `bool` (was `Result<(), Error>`, now `Result<bool, Error>`). The caller (`parse_worksheet`) only increments and bounds-checks the cell counter when it did — an empty cell costs zero memory, so counting it too would wrongly restrict a legitimately sparse file. The counting/bounds-check logic itself is factored into a small separate function, `check_cell_count`, called from both of `flush_cell`'s two call sites (the self-closing `<c/>` case and the `Event::End` case).

## Open Questions

1. ~~Reconsidering where `PendingSharedString` / `PendingStyle` live~~ → **Resolved**: both types' definitions were relocated to this file (`parse/worksheet.rs`), with [`resolve/shared_strings.rs`](../resolve/shared_strings.en.md) / [`resolve/style.rs`](../resolve/style.en.md) each `use`-ing them (reflects the [PR #9 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204)). See Dependencies for details.
2. **Handling formulas (`<f>` elements)**: currently assumes their content is never parsed or retained at all (only `<v>`'s cached computed value is used). If a future requirement needs the formula text itself in the JSON output, whether to add `formula: Option<String>` to `Cell` or otherwise is to be settled together with a more detailed requirements pass.
3. **Fallback policy for an unrecognized `t` attribute value**: currently falls back to keeping the raw `<v>` text as `CellValue::Text` as-is. This follows the same philosophy as [parse/workbook.md](workbook.en.md)'s `state`-attribute fallback (err on the side of not losing data), but there is a case for treating it as a hard `Error` instead.
4. ~~Sequential column-position inference for cells omitting `r`~~ → **Resolved** (Issue [#79](https://github.com/MinamiyamaKotaro/xlsxparser/issues/79)): both `<row>` and `<c>` now infer an omitted `r` from the previous row/cell. Added `cur_row`/`cur_col` as loop-local state: a `<row>` start tag settles the row number and resets the column counter; each `<c>` start tag either adopts its own `r` (updating the current column from it, so a later omitted cell resumes from there) or advances the current column by one when `r` is absent. Either path applies the same `CellRef::MAX_ROW`/`MAX_COL` bound as an explicit `r` (security review Finding 2 — a run of `r`-omitting `<c>` elements would otherwise never pass through `CellRef::from_a1`'s own check, letting it inflate `Sheet::max_col` unbounded). The real file this open question already cited, `tests/fixtures/other/minimal_package.xlsx` (calamine's test corpus, not committed), was confirmed to now resolve end to end with this fix.
5. **`Reader` internal buffer size / performance tuning**: same topic as [parse/mod.md Open Question 5](mod.en.md). To be settled based on measured profiling against the "grid-paper Excel" sheet sizes the requirements target.
6. ~~Namespace handling~~ → **Resolved**: follows the policy [parse/mod.md Open Question 4](mod.en.md) settled on — plain string-prefix matching, no `quick_xml::NsReader`. `worksheet.xml`'s own elements and attributes (`row`, `c`, `v`, `is`, `t`, `s`, `r`, `mergeCells`, `mergeCell`, `ref`) carry no prefix, so this file sees no direct impact.
7. ~~Where and how to check the cell-count cap~~ → **Resolved** (Issue [#88](https://github.com/MinamiyamaKotaro/xlsxparser/issues/88)): adopted incremental counting and checking while this function streams `<c>` elements, rather than the "check in one batch after collection" pattern `resolve::merge`/`resolve::column_width` use. Rationale: for cells, the memory cost accrues the moment one reaches `Sheet::insert_cell`, so checking only after collecting the whole sheet would already be too late. The cap value itself is caller-configurable via `SizeLimits::max_cells_per_sheet` ([container/sanitize.md](../container/sanitize.en.md)), bridged in through `parse_worksheet`'s new `max_cells` argument, which `pipeline.rs` forwards.
