# `json.rs` Design Doc

*[日本語](json.md)*

Design doc for `src/json.rs`. This implements Phase 5, "JSON generation (return)," as defined by [architecture.md](architecture.en.md). It serializes the fully analyzed and resolved [`model::Workbook`](model/workbook.en.md) into JSON carrying attributes such as `row_span` / `col_span` needed for frontend rendering (requirements chapter 5).

## Responsibility / Scope

- Converts [`model::Workbook`](model/workbook.en.md) into a serialization-only JSON DTO (`JsonWorkbook`)
- Computes `row_span` / `col_span` using [`Sheet::iter_cells`](model/sheet.en.md) (which iterates only origin cells) and [`Sheet::merged_region_at`](model/sheet.en.md), and never includes a merged cell's virtual coordinates in the JSON output (implements requirements 3.2 and chapter 5)
- Emits, for each [`CellValue`](model/cell.en.md) variant, the JSON value along with a kind tag (`type: "number" | "text" | "boolean" | "error" | "dateTime"`; a valueless cell is `"empty"`)
- **Not responsible for**: resolving or validating model data itself (`resolve/` — by the time data reaches this file, `Workbook` holds only valid data that has already passed every phase's validation), actually writing the produced JSON string out to a file, HTTP response, etc. (the caller's responsibility)

## Key Types / Functions (draft)

```rust
use crate::model::cell::{Cell, CellRef, CellValue};
use crate::model::sheet::{Sheet, SheetVisibility};
use crate::model::workbook::Workbook;
use serde::Serialize;

/// A serialization-only DTO for JSON output. See Dependencies for why this
/// is not simply `#[derive(Serialize)]` directly on `model::Workbook`.
#[derive(Debug, Serialize)]
pub struct JsonWorkbook {
    pub sheets: Vec<JsonSheet>,
}

#[derive(Debug, Serialize)]
pub struct JsonSheet {
    pub name: String,
    pub visibility: &'static str, // "visible" | "hidden" | "veryHidden"
    #[serde(rename = "maxRow")]
    pub max_row: u32,
    #[serde(rename = "maxCol")]
    pub max_col: u32,
    pub cells: Vec<JsonCell>,
}

#[derive(Debug, Serialize)]
pub struct JsonCell {
    pub row: u32,
    pub col: u32,
    pub value: JsonCellValue,
    /// Omitted entirely when 1 (not merged).
    #[serde(rename = "rowSpan", skip_serializing_if = "is_one")]
    pub row_span: u32,
    #[serde(rename = "colSpan", skip_serializing_if = "is_one")]
    pub col_span: u32,
    // Style output (font, fill, etc.) is blocked on ResolvedStyle gaining
    // those fields — see Open Question 4.
}

fn is_one(n: &u32) -> bool {
    *n == 1
}

/// A kind-tagged value representation. `#[serde(tag = "type", content =
/// "value")]` serializes as `{"type": "number", "value": 42.0}` (whether
/// tagging is the right call is discussed in Open Question 1).
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum JsonCellValue {
    Number(f64),
    /// The concrete string representation (ISO 8601, etc.) is to be decided
    /// once `model::DateTimeValue`'s type is settled — see Open Question 3.
    DateTime(String),
    Text(std::sync::Arc<str>),
    Boolean(bool),
    Error(String),
    /// A cell with no value (formatting only).
    Empty,
}

/// Converts `workbook` into the JSON DTO. Returns no `Result`, since by the
/// time it arrives here `workbook` holds only valid data that has already
/// passed every phase's validation — see Error Handling Policy.
pub fn to_json_workbook(workbook: &Workbook) -> JsonWorkbook {
    JsonWorkbook {
        sheets: workbook.sheets().iter().map(sheet_to_json).collect(),
    }
}

fn sheet_to_json(sheet: &Sheet) -> JsonSheet {
    JsonSheet {
        name: sheet.name.clone(),
        visibility: visibility_tag(sheet.visibility),
        max_row: sheet.max_row,
        max_col: sheet.max_col,
        cells: sheet
            .iter_cells()
            .map(|(cell_ref, cell)| cell_to_json(sheet, cell_ref, cell))
            .collect(),
    }
}

fn cell_to_json(sheet: &Sheet, cell_ref: CellRef, cell: &Cell) -> JsonCell {
    let (row_span, col_span) = sheet
        .merged_region_at(cell_ref)
        .map(|r| (r.row_span(), r.col_span()))
        .unwrap_or((1, 1));
    JsonCell {
        row: cell_ref.row,
        col: cell_ref.col,
        value: cell_value_to_json(cell.value.as_ref()),
        row_span,
        col_span,
    }
}

fn cell_value_to_json(value: Option<&CellValue>) -> JsonCellValue {
    match value {
        None => JsonCellValue::Empty,
        Some(CellValue::Number(n)) => JsonCellValue::Number(sanitize_float(*n)),
        Some(CellValue::DateTime(dt)) => JsonCellValue::DateTime(format_date_time(dt)),
        Some(CellValue::Text(s)) => JsonCellValue::Text(s.clone()),
        Some(CellValue::Boolean(b)) => JsonCellValue::Boolean(*b),
        Some(CellValue::Error(e)) => JsonCellValue::Error(e.clone()),
    }
}

/// JSON cannot represent NaN/Infinity, so non-finite f64 values fall back
/// to 0.0 (whether this is a sound substitute — see Open Question 2).
fn sanitize_float(n: f64) -> f64 {
    if n.is_finite() { n } else { 0.0 }
}

/// Converts a `model::DateTimeValue` (its type still undecided — see
/// [model/cell.md Open Question 4](model/cell.en.md)) into its string
/// representation. The concrete format is to be decided together with that
/// type.
fn format_date_time(dt: &crate::model::cell::DateTimeValue) -> String {
    let _ = dt;
    unimplemented!()
}

fn visibility_tag(v: SheetVisibility) -> &'static str {
    match v {
        SheetVisibility::Visible => "visible",
        SheetVisibility::Hidden => "hidden",
        SheetVisibility::VeryHidden => "veryHidden",
    }
}
```

## Dependencies

- Depends on: [`model/workbook.rs`](model/workbook.en.md) (`Workbook`), [`model/sheet.rs`](model/sheet.en.md) (`Sheet::iter_cells`, `Sheet::merged_region_at`, `SheetVisibility`), [`model/cell.rs`](model/cell.en.md) (`Cell`, `CellRef`, `CellValue`, `DateTimeValue`), the external `serde` crate (deriving `Serialize`)
- Depended on by: `lib.rs` (calls it explicitly on a `Workbook` — see [pipeline.md Open Question 1](pipeline.en.md); `pipeline.rs`'s `run` itself never calls it)

`model::Workbook` / `Sheet` / `Cell` are deliberately not annotated with `#[derive(Serialize)]` directly; instead this file converts them into its own DTOs (`JsonWorkbook`, etc.) before serializing. The reasoning mirrors [error.md](error.en.md)'s design decision to type-erase `Error::XmlParse::source` so `quick-xml` never becomes a public dependency: pulling a `serde` dependency into `model/` would let `serde`'s breaking changes ripple into `model/`'s own type definitions. Interposing a DTO conversion layer keeps `model/` as architecture.md's policy intends — "a pure data structure with no dependency on XML parsing or resolution logic" — and lets the JSON output's concrete field names and shape (camelCasing, whether to tag kinds, etc.) evolve independently of `model/`'s type definitions.

## Error Handling Policy

- This file's conversion functions never return `Result`. Reason: by the time data reaches this file, `model::Workbook` holds only valid data that has already passed Phases 1–4 (if there were a parse or validation error, [`pipeline.rs`](pipeline.en.md) would have already returned `Err` at that point, and execution would never reach this file), so this file's own conversion logic has no external factor that could make it fail (e.g. reinterpreting untrusted input)
- The one value JSON cannot represent — `f64`'s `NaN`/`Infinity` (which `CellValue::Number` can in theory hold) — falls back to `0.0` rather than erroring. This follows the same principle [resolve/style.md](resolve/style.en.md)'s `serial_to_date_time` already adopted: "a loose failure interpreting an individual value should not fail the whole document" (whether `0.0` is a sound substitute — see Open Question 2)

## Testing Strategy

- Verify that a `Workbook` with a single sheet and a single (numeric) cell converts correctly to `JsonWorkbook`
- Verify that for a sheet with a merged cell, the origin cell's `rowSpan`/`colSpan` are computed correctly and the virtual cell coordinates are never included in the `cells` array (wiring to `Sheet::iter_cells`'s origin-only design)
- Verify that for an unmerged, ordinary cell, the `rowSpan`/`colSpan` fields are omitted from the serialized output (`skip_serializing_if`)
- Verify that each `CellValue` variant (`Number`/`Text`/`Boolean`/`Error`) serializes correctly with `JsonCellValue`'s corresponding `type` tag and `value`
- Verify that `value: None` (a formatting-only cell) serializes as `type: "empty"`
- Verify that a cell holding `CellValue::Number(f64::NAN)` / `CellValue::Number(f64::INFINITY)` never panics and is output as `0.0` (a regression test for Open Question 2)
- Verify that for a `Workbook` containing `Hidden`/`VeryHidden` sheets, every sheet appears in the output with its `visibility` field (wiring to [model/workbook.md](model/workbook.en.md)'s "include every sheet regardless of visibility" policy)
- Verify that a `Workbook` with zero sheets serializes correctly as `{"sheets": []}`

## Open Questions

1. **Whether to tag value kinds in the JSON structure**: currently adopts a tagged representation like `{"type": "number", "value": 42}`, but for a frontend that simply displays `value` as-is, there is a case that outputting native JSON types directly (a number as `number`, a string as `string`, etc.) without a tag would be simpler. However, that would leave no way for the frontend to distinguish `DateTime` from `Number`, so which to prioritize is to be settled together with a more detailed elaboration of the requirements' frontend use case.
2. **Fallback value for non-finite floating-point numbers (`NaN`/`Infinity`)**: currently substitutes `0.0`, which is indistinguishable from the original value and could mislead. Falling back to `null` (equivalent to `JsonCellValue::Empty`), or outputting a string (`"NaN"`, etc.), are alternatives worth considering.
3. **`DateTime`'s string representation format**: undecided, tied to [model/cell.md Open Question 4](model/cell.en.md)'s finalization of the `DateTimeValue` type. ISO 8601 (e.g. `"2024-01-01T00:00:00"`) is the leading candidate, but how to handle date-only or time-only cells (Excel does not distinguish date/time precision as a type) needs consideration.
4. **JSON output of style information**: same topic as [model/style.md Open Question 1](model/style.en.md). Style output fields cannot be added to `JsonCell` until `ResolvedStyle` gains concrete font/fill/border fields.
5. **Peak memory from batch construction**: given the requirements' focus on "grid-paper Excel"-scale large data, the current design of building `Vec<JsonCell>` in one batch per sheet before serializing with `serde_json` increases peak memory usage for a very large single sheet. Whether there's room to switch to a design that writes cells out incrementally — via `serde_json::to_writer` and manually-driven streaming serialization (`SerializeSeq`, etc.) — is to be settled together with performance requirements.
6. **Relationship between `to_json_workbook` and `lib.rs`'s public API**: how `lib.rs` exposes this function (or a wrapper producing a JSON string) separately from `parse_workbook` (which returns `Workbook`) is tied to [pipeline.md Open Question 1](pipeline.en.md) and is to be settled when `lib.rs` is designed.
