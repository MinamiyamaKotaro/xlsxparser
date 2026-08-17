# `parse/worksheet.rs` Design Doc

*[日本語](worksheet.md)*

Design doc for `src/parse/worksheet.rs`. Per [architecture.md](../architecture.en.md), this file *is* Phase 3: "SAX-style streaming parse of `sheetX.xml` (row-level disposal is completed here)." To satisfy requirements chapter 3's core functionality (sparse-matrix memory optimization, transparent access to merged cells), it streams cells into [`model::Sheet::insert_cell`](../model/sheet.en.md) while shaping shared-string, style, and merge-range information that needs deferred resolution into the form [`resolve/`](../resolve/mod.en.md) (Phase 4) consumes. This file finalizes the contract that [resolve/mod.md](../resolve/mod.en.md), [resolve/shared_strings.md](../resolve/shared_strings.en.md), [resolve/style.md](../resolve/style.en.md), and [resolve/merge.md](../resolve/merge.en.md) had all assumed while noting "`parse/worksheet.rs` is not yet designed."

## Responsibility / Scope

- Streams `xl/worksheets/sheetX.xml`'s `<sheetData>` row (`<row>`) by row, building a [`Cell`](../model/cell.en.md) for each `<c>` and inserting it via `Sheet::insert_cell`
- Once one row's worth of data is fully processed (every `<c>` belonging to that row has been read and reflected via `insert_cell`), discards the parser's internal state for that row (attributes, text buffers, etc.) before moving on to the next row (implements the requirements' Phase 3 requirement; per architecture.md, "row-level XML node disposal is an internal implementation detail of `parse/worksheet.rs`; `pipeline.rs` does not control it")
- When a `t="s"` (shared-string index reference) cell is detected, inserts a `Cell` with `value: None` via `insert_cell` while simultaneously recording a corresponding [`resolve::PendingSharedString`](../resolve/shared_strings.en.md)
- When a cell carries an `s` (`cellXfs` index — style ID reference) attribute, records a corresponding [`resolve::PendingStyle`](../resolve/style.en.md)
- `t="str"` (formula-computed string result) and `t="inlineStr"` (inline string) cells need no deferred resolution, so they are resolved directly to `CellValue::Text` during the stream and inserted via `insert_cell` (the division of labor already assumed by [resolve/shared_strings.md Responsibility/Scope](../resolve/shared_strings.en.md))
- Collects `<mergeCells><mergeCell ref="A1:C3"/>...</mergeCells>`, which appears after the stream completes (after `</sheetData>`), converting each `ref` into `start`/`end` via [`CellRef::from_a1`](../model/cell.en.md) and gathering them into `Vec<MergedRegion>` (the stage that feeds into [`resolve/merge.rs`](../resolve/merge.en.md)'s validation and registration)
- **Not responsible for**: actually resolving shared-string indices or style IDs ([`resolve/shared_strings.rs`](../resolve/shared_strings.en.md) / [`resolve/style.rs`](../resolve/style.en.md)), validating merge ranges or registering them into `Sheet` ([`resolve/merge.rs`](../resolve/merge.en.md) — this file only collects the `MergedRegion` list; it never calls `insert_merge`), parsing or retaining formulas (`<f>` elements — see Open Question 2)

## Key Types / Functions (draft)

```rust
use crate::error::Error;
use crate::model::cell::{Cell, CellRef, CellValue};
use crate::model::sheet::{MergedRegion, Sheet};
use crate::parse::{concat_rich_text, convert_xml_error, create_secure_reader, required_attr};
use crate::resolve::{PendingSharedString, PendingStyle};
use std::io::BufRead;
use std::sync::Arc;

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
pub(crate) fn parse_worksheet(
    reader: impl BufRead,
    path: &str,
    sheet: &mut Sheet,
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
) -> Result<Cell, Error> {
    let value = match cell_type {
        None | Some("n") => value_text.map(parse_number).transpose()?.map(CellValue::Number),
        Some("s") => None, // resolved by resolve/shared_strings.rs (recorded separately as a PendingSharedString)
        Some("str") => value_text.map(|s| CellValue::Text(Arc::from(s))),
        Some("inlineStr") => inline_string.map(|s| CellValue::Text(Arc::from(s))),
        Some("b") => value_text.map(|s| s == "1").map(CellValue::Boolean),
        Some("e") => value_text.map(|s| CellValue::Error(s.to_string())),
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
```

## Dependencies

- Depends on: [`parse/mod.rs`](mod.en.md) (`create_secure_reader`, `convert_xml_error`, `required_attr`, `concat_rich_text`), [`model/cell.rs`](../model/cell.en.md) (`Cell`, `CellRef`, `CellValue`), [`model/sheet.rs`](../model/sheet.en.md) (`Sheet::insert_cell`, `MergedRegion`), [`resolve/mod.rs`](../resolve/mod.en.md) (re-exports of `PendingSharedString`, `PendingStyle`), [`error.rs`](../error.en.md)
- Depended on by: `pipeline.rs` (Phase 3 — called once per sheet, passing the return value straight through to [`resolve::resolve_sheet`](../resolve/mod.en.md))

**A design wrinkle surfaced while writing this file (see Open Question 1)**: because this file constructs `PendingSharedString` / `PendingStyle` directly (types defined by [`resolve/shared_strings.rs`](../resolve/shared_strings.en.md) / [`resolve/style.rs`](../resolve/style.en.md)), `parse::worksheet` ends up depending on `resolve::shared_strings` / `resolve::style`. Meanwhile [resolve/mod.md Dependencies](../resolve/mod.en.md) had already committed to `resolve/mod.rs` depending on `parse::shared_strings::SharedStringTable`. Laid out, the dependency direction is as follows, and it stays acyclic (a DAG):

```text
parse::worksheet ─┬─▶ resolve::mod ─▶ resolve::shared_strings ─▶ parse::shared_strings
                   └─▶ resolve::mod ─▶ resolve::style
```

`parse::shared_strings` itself remains a leaf module with no dependency on either `parse::worksheet` or `resolve::mod`, so the path `parse::worksheet → resolve::* → parse::shared_strings` does not form a cycle. That said, having a module under `parse/` `use` types from `resolve/` directly runs somewhat against the spirit of architecture.md design policy 2 — separating the I/O layer (`container`/`parse`) from domain logic (`resolve`). This is the same kind of structural wrinkle that [model/style.rs](../model/style.en.md) already fixed once for `ResolvedStyle`/`StyleSheet` by relocating them out of `resolve/style.rs` into `model/`, breaking the direct dependency between `parse/styles.rs` and `resolve/style.rs` (per the PR #8 review). This design defers changing the already-settled type definitions and placement in [resolve/mod.md](../resolve/mod.en.md) / [resolve/shared_strings.md](../resolve/shared_strings.en.md) / [resolve/style.md](../resolve/style.en.md) — that's outside this Issue's scope — and instead records the dependency explicitly here, leaving it as Open Question 1 for a future review.

## Error Handling Policy

- Structurally invalid XML is converted into `Error::XmlParse` or `Error::ZipBombDetected` via [`convert_xml_error`](mod.en.md)
- A `<c>` missing its `r` attribute (the cell reference, e.g. `"B12"`) returns `Error::MissingRequiredElement`. This design does not perform sequential column-position inference for cells omitted within a row (the schema technically permits inferring the next column from the previous cell when `r` is absent) — see Open Question 4
- If `r`'s value is not well-formed A1 notation (`CellRef::from_a1` returns `Err`), that `Error::InvalidCellRef` propagates unchanged
- If `<v>`'s numeric text cannot be parsed as `f64`, this returns `Error::InvalidPackage` (provisional; whether a more specific variant is warranted is left to a future revision of [error.md](../error.en.md))
- If a `<mergeCell ref="...">`'s `ref` value is not in the `"A1:C3"` shape (two `:`-separated coordinates), or either coordinate is not well-formed A1 notation, `Error::InvalidCellRef` propagates. This file does not validate the range's soundness itself (start/end ordering, overlap with other ranges) — it simply appends to `merge_regions` and leaves validation to [`resolve/merge.rs`](../resolve/merge.en.md)
- `<f>` (formula) element content is neither parsed nor retained — it is skipped (outside the requirements' scope; see Open Question 2)
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
- Verify that a `<c>` missing `r` returns `Error::MissingRequiredElement`
- Verify that a malformed A1-notation `r` attribute or `mergeCell ref` attribute returns `Error::InvalidCellRef`
- Verify that for a cell containing an `<f>` element (a formula cell), the `<f>` content is ignored and only `<v>` (the cached computed value) is used as the `Cell`'s value

## Open Questions

1. **Reconsidering where `PendingSharedString` / `PendingStyle` live**: as discussed under Dependencies, having `parse::worksheet` `use` `resolve::shared_strings` / `resolve::style` types directly stays acyclic but runs somewhat against the spirit of architecture.md design policy 2 (separating the I/O layer from domain logic). Following the precedent of [model/style.rs](../model/style.en.md) relocating `ResolvedStyle`/`StyleSheet` out of `resolve/style.rs` into `model/`, whether `PendingSharedString`/`PendingStyle` should likewise move to `resolve/mod.rs` (or a more neutral location) is treated as out of this Issue's scope, since it would require revisiting [resolve/mod.md](../resolve/mod.en.md), [resolve/shared_strings.md](../resolve/shared_strings.en.md), and [resolve/style.md](../resolve/style.en.md) — left for a separate future review.
2. **Handling formulas (`<f>` elements)**: currently assumes their content is never parsed or retained at all (only `<v>`'s cached computed value is used). If a future requirement needs the formula text itself in the JSON output, whether to add `formula: Option<String>` to `Cell` or otherwise is to be settled together with a more detailed requirements pass.
3. **Fallback policy for an unrecognized `t` attribute value**: currently falls back to keeping the raw `<v>` text as `CellValue::Text` as-is. This follows the same philosophy as [parse/workbook.md](workbook.en.md)'s `state`-attribute fallback (err on the side of not losing data), but there is a case for treating it as a hard `Error` instead.
4. **Sequential column-position inference for cells omitting `r`**: per the OOXML spec, a `<c>`'s `r` attribute is optional, and when absent, sequential inference of column position from the preceding cell is permitted. This design currently does not support that, adopting the simplification of returning `Error::MissingRequiredElement` instead. Given [model/sheet.md](../model/sheet.en.md)'s already-noted concern that "`.xlsx` files generated by third-party tools may rely on looser parts of the spec," support may become necessary if a generator that actually omits `r` is encountered in practice.
5. **`Reader` internal buffer size / performance tuning**: same topic as [parse/mod.md Open Question 5](mod.en.md). To be settled based on measured profiling against the "grid-paper Excel" sheet sizes the requirements target.
6. **Namespace handling**: same topic as [parse/mod.md Open Question 4](mod.en.md). `worksheet.xml`'s own elements and attributes (`row`, `c`, `v`, `is`, `t`, `s`, `r`, `mergeCells`, `mergeCell`, `ref`) carry no prefix, so this file is expected to see no direct impact.
