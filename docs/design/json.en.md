# `json.rs` Design Doc

*[日本語](json.md)*

Design doc for `src/json.rs`. This implements Phase 5, "JSON generation (return)," as defined by [architecture.md](architecture.en.md). It serializes the fully analyzed and resolved [`model::Workbook`](model/workbook.en.md) into JSON carrying attributes such as `row_span` / `col_span` needed for frontend rendering (requirements chapter 5).

## Responsibility / Scope

- Serializes [`model::Workbook`](model/workbook.en.md) into JSON that includes `row_span`/`col_span` and a value kind tag
- Writes cells one at a time, directly to the serializer, from the iterator [`Sheet::iter_cells`](model/sheet.en.md) returns (which iterates only origin cells), never building an intermediate `Vec` for a whole sheet on the heap (reflects the [PR #10 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/10#pullrequestreview-4949223332), resolving Open Question 5 — keeps peak memory in check for the "grid-paper Excel"-scale sheets the requirements target)
- Computes `row_span`/`col_span` using [`Sheet::merged_region_at`](model/sheet.en.md), never including a merged cell's virtual coordinates in the JSON output (implements requirements 3.2 and chapter 5)
- Emits, for each [`CellValue`](model/cell.en.md) variant, the JSON value along with a kind tag (`type: "number" | "text" | "boolean" | "error" | "dateTime"`; a valueless cell, or a value that cannot be represented in JSON, falls back to `"empty"`)
- **Not responsible for**: resolving or validating model data itself (`resolve/` — by the time data reaches this file, `Workbook` holds only valid data that has already passed every phase's validation), preparing the `Write` implementation passed to `to_json_writer` itself (opening a file, obtaining an HTTP response body, etc. — the caller's responsibility)

## Key Types / Functions (draft)

```rust
use crate::error::Error;
use crate::model::cell::{Cell, CellRef, CellValue};
use crate::model::sheet::{Sheet, SheetVisibility};
use crate::model::style::Alignment;
use crate::model::workbook::Workbook;
use serde::ser::{SerializeSeq, SerializeStruct};
use serde::{Serialize, Serializer};
use std::io::Write;

/// Streams `workbook` out as JSON to `writer`. Each element of the `cells`
/// array is converted from [`Sheet::iter_cells`](model/sheet.en.md) and
/// written out one at a time, without buffering a whole sheet's
/// `Vec<JsonCell>` in memory. If `writer` is, say, a `BufWriter<File>`,
/// additional memory usage stays at O(1) — one cell's worth at a time.
pub fn to_json_writer<W: Write>(workbook: &Workbook, writer: W) -> Result<(), Error> {
    let json_workbook = JsonWorkbook { workbook };
    serde_json::to_writer(writer, &json_workbook)
        .map_err(|source| Error::JsonSerialize { source: Box::new(source) })
}

/// A convenience version of `to_json_writer` that targets an in-memory
/// `Vec<u8>`. Since the entire output must be held as one `String`,
/// additional memory usage is O(n) in the output size (unlike
/// `to_json_writer`'s O(1) — prefer `to_json_writer` whenever the caller
/// can write directly to a file, HTTP response, etc.).
pub fn to_json_string(workbook: &Workbook) -> Result<String, Error> {
    let mut buf = Vec::new();
    to_json_writer(workbook, &mut buf)?;
    // serde_json is guaranteed to always emit valid UTF-8, so this
    // conversion cannot fail in practice, but per the library-wide policy
    // of never using `unwrap`/`expect` internally (error.md Error Handling
    // Policy), it is still handled as a `Result`.
    String::from_utf8(buf).map_err(|source| Error::JsonSerialize { source: Box::new(source) })
}

/// A borrowing wrapper over `model::Workbook`. Owns no value; its
/// `Serialize` impl walks the model on demand, achieving streaming (not
/// exposed outside this file).
struct JsonWorkbook<'a> {
    workbook: &'a Workbook,
}

impl<'a> Serialize for JsonWorkbook<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Workbook", 1)?;
        state.serialize_field("sheets", &SheetSeq { workbook: self.workbook })?;
        state.end()
    }
}

struct SheetSeq<'a> {
    workbook: &'a Workbook,
}

impl<'a> Serialize for SheetSeq<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let sheets = self.workbook.sheets();
        let mut seq = serializer.serialize_seq(Some(sheets.len()))?;
        for sheet in sheets {
            seq.serialize_element(&JsonSheet { sheet })?;
        }
        seq.end()
    }
}

struct JsonSheet<'a> {
    sheet: &'a Sheet,
}

impl<'a> Serialize for JsonSheet<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Sheet", 7)?;
        state.serialize_field("name", &self.sheet.name)?;
        state.serialize_field("visibility", visibility_tag(self.sheet.visibility))?;
        state.serialize_field("maxRow", &self.sheet.max_row)?;
        state.serialize_field("maxCol", &self.sheet.max_col)?;
        // `defaultColumnWidth`/`columns` (Issue #39): a sheet-level array,
        // not one `columnWidth` value duplicated onto every cell in that
        // column — see model/sheet.en.md's "Feature: column width" note
        // for why (raised during Issue #36's review discussion).
        state.serialize_field("defaultColumnWidth", &self.sheet.default_col_width())?;
        state.serialize_field("columns", &ColumnSeq { sheet: self.sheet })?;
        state.serialize_field("cells", &CellSeq { sheet: self.sheet })?;
        state.end()
    }
}

/// Converts each cell from [`Sheet::iter_cells`](model/sheet.en.md) into a
/// `JsonCell` and writes it straight to the serializer, one at a time,
/// without ever building an intermediate `Vec<JsonCell>` (the core of the
/// design that resolves Open Question 5).
struct CellSeq<'a> {
    sheet: &'a Sheet,
}

impl<'a> Serialize for CellSeq<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Sheet::iter_cells is exposed only as `impl Iterator` and makes no
        // size-hint guarantee, so `None` is used here (whether to commit
        // `ExactSizeIterator` as part of the public API is a design
        // decision left to model/sheet.md — see Open Question 5).
        let mut seq = serializer.serialize_seq(None)?;
        for (cell_ref, cell) in self.sheet.iter_cells() {
            seq.serialize_element(&cell_to_json(self.sheet, cell_ref, cell))?;
        }
        seq.end()
    }
}

/// The conversion result for a single cell. `CellSeq::serialize` produces
/// one of these per stream element, short-lived (not exposed to callers).
#[derive(Debug, Serialize)]
struct JsonCell {
    row: u32,
    col: u32,
    value: JsonCellValue,
    /// Omitted entirely when 1 (not merged).
    #[serde(rename = "rowSpan", skip_serializing_if = "is_one")]
    row_span: u32,
    #[serde(rename = "colSpan", skip_serializing_if = "is_one")]
    col_span: u32,
    /// Omitted entirely when the cell has no style at all (`Cell.style:
    /// None`). Unlike `columns` (a sheet-level array — see
    /// model/sheet.en.md's "Feature: column width" note), font genuinely
    /// varies cell-to-cell within a column, so embedding it per cell has
    /// no sparse-output principle to violate (Issue #38).
    #[serde(skip_serializing_if = "Option::is_none")]
    style: Option<JsonStyle>,
}

fn is_one(n: &u32) -> bool {
    *n == 1
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonStyle {
    font: JsonFont,
    wrap_text: bool,
    /// Always present, like `font`/`wrap_text` (never `Option`) — unlike
    /// `numberFormat`, "general" is a real, meaningful alignment mode, not
    /// "nothing to report" (Issue #42).
    alignment: &'static str,
    /// Omitted when `None` ("General" — no special format; see
    /// `model/style.rs`'s `ResolvedStyle::number_format` doc comment for why
    /// this is skipped rather than emitted as `"General"`) — unlike `font`/
    /// `wrap_text`/`alignment`, which always carry a meaningful value once a
    /// `style` object exists at all (Issue #41).
    #[serde(skip_serializing_if = "Option::is_none")]
    number_format: Option<String>,
    /// Omitted when the `<fill>` carries no `<fgColor>`/`<bgColor>` at all
    /// (Issue #75) — same "nothing to report" treatment as `number_format`.
    #[serde(skip_serializing_if = "Option::is_none")]
    fill_fg_color: Option<JsonColorRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fill_bg_color: Option<JsonColorRef>,
    /// Omitted when no side carries a border at all (`Borders::any()` is
    /// `false` — most cells) rather than emitted as
    /// `{"top":false,"right":false,"bottom":false,"left":false}` (Issue
    /// #97) — same "nothing to report" treatment as `fillFgColor`.
    #[serde(skip_serializing_if = "Option::is_none")]
    borders: Option<JsonBorders>,
}

/// `Borders`'s JSON form (Issue #97) — a plain, non-tagged object (unlike
/// `JsonColorRef`, no variant to distinguish). All four fields are always
/// present together when the object itself is present at all (mirrors
/// `rowSpan`/`colSpan`'s single-value "all or nothing" omission, not
/// `fillFgColor`/`fillBgColor`'s per-field omission — a per-side `false`
/// is meaningful information here, not "nothing to report").
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonBorders {
    top: bool,
    right: bool,
    bottom: bool,
    left: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonFont {
    size_pt: f64,
    bold: bool,
}

/// `ColorRef`'s JSON form (Issue #75), tagged the same way `JsonCellValue`
/// is — e.g. `{"type":"theme","value":{"index":4,"tint":-0.25}}`. Kept
/// raw/unresolved, same as `model::ColorRef` itself.
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
enum JsonColorRef {
    Rgb(String),
    Theme { index: u32, tint: Option<f64> },
    Indexed(u32),
}

/// A kind-tagged value representation. `#[serde(tag = "type", content =
/// "value")]` serializes as `{"type": "number", "value": 42.0}`.
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
enum JsonCellValue {
    Number(f64),
    /// ISO 8601, no fractional seconds (`DateTimeValue` doesn't carry
    /// sub-second precision), e.g. `"2024-01-01T13:45:30"` (Issue #40,
    /// resolving Open Question 3). A date-only cell serializes with a
    /// midnight time component, since Excel itself doesn't distinguish
    /// date-only from date+time as a type.
    DateTime(String),
    Text(std::sync::Arc<str>),
    Boolean(bool),
    Error(String),
    /// A cell with no value (formatting only), or the fallback destination
    /// for a value JSON cannot represent (non-finite floats — see below).
    Empty,
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
        style: cell.style.as_ref().map(|s| JsonStyle {
            font: JsonFont { size_pt: s.font.size_pt, bold: s.font.bold },
            wrap_text: s.wrap_text,
            alignment: alignment_tag(s.horizontal_alignment),
            number_format: s.number_format.as_deref().map(str::to_string),
            fill_fg_color: s.fill_fg_color.as_ref().map(color_ref_to_json),
            fill_bg_color: s.fill_bg_color.as_ref().map(color_ref_to_json),
            borders: borders_to_json(&s.borders),
        }),
    }
}

fn borders_to_json(b: &Borders) -> Option<JsonBorders> {
    b.any().then_some(JsonBorders {
        top: b.top,
        right: b.right,
        bottom: b.bottom,
        left: b.left,
    })
}

fn cell_value_to_json(value: Option<&CellValue>) -> JsonCellValue {
    match value {
        None => JsonCellValue::Empty,
        Some(CellValue::Number(n)) if n.is_finite() => JsonCellValue::Number(*n),
        // Silently substituting 0.0 for NaN/Infinity would make it
        // indistinguishable, downstream, from a value that legitimately
        // evaluated to zero, risking incorrect aggregation results (given
        // requirements chapter 1's accounting/business-system use case;
        // reflects the PR #10 review, resolving Open Question 2). Falling
        // back to Empty (JSON `null`) instead lets the frontend safely
        // treat it as "no value present."
        Some(CellValue::Number(_)) => JsonCellValue::Empty,
        Some(CellValue::DateTime(dt)) => JsonCellValue::DateTime(format_date_time(dt)),
        Some(CellValue::Text(s)) => JsonCellValue::Text(s.clone()),
        Some(CellValue::Boolean(b)) => JsonCellValue::Boolean(*b),
        Some(CellValue::Error(e)) => JsonCellValue::Error(e.clone()),
    }
}

/// Formats a `DateTimeValue` as ISO 8601 without a timezone designator or
/// fractional seconds, e.g. `"2024-01-01T13:45:30"`.
fn format_date_time(dt: &crate::model::cell::DateTimeValue) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
    )
}

/// `model::style::Alignment` doesn't derive `Serialize` directly (keeping
/// `serde` out of `model/`'s dependency surface — see Dependencies below),
/// so this mirrors `visibility_tag`'s pattern instead (Issue #42).
fn alignment_tag(a: Alignment) -> &'static str {
    match a {
        Alignment::General => "general",
        Alignment::Left => "left",
        Alignment::Center => "center",
        Alignment::Right => "right",
        Alignment::Fill => "fill",
        Alignment::Justify => "justify",
        Alignment::CenterContinuous => "centerContinuous",
        Alignment::Distributed => "distributed",
    }
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

- Depends on: [`model/workbook.rs`](model/workbook.en.md) (`Workbook`), [`model/sheet.rs`](model/sheet.en.md) (`Sheet::iter_cells`, `Sheet::merged_region_at`, `Sheet::images`, `SheetVisibility`, `Image`, `ImageAnchor`, `AnchorMarker` — Issue #65), [`model/cell.rs`](model/cell.en.md) (`Cell`, `CellRef`, `CellValue`, `DateTimeValue`), [`model/style.rs`](model/style.en.md) (`Alignment` — read via `s.horizontal_alignment` in `cell_to_json`, converted through `alignment_tag` rather than deriving `Serialize` directly, per this file's own no-`serde`-in-`model/` policy below; `ColorRef` — read via `s.fill_fg_color`/`fill_bg_color`, converted through `color_ref_to_json` the same way; `Borders` — read via `s.borders`, its four `bool` fields copied directly into `JsonBorders` since a plain `bool` needs no `model`→JSON conversion function the way `ColorRef`/`Alignment` do), [`error.rs`](error.en.md) (`Error::JsonSerialize` — newly added to represent I/O or serialization failure during streaming writes; added as part of the redesign following the [PR #10 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/10#pullrequestreview-4949223332)), the external `serde` crate (manual and derived `Serialize` impls) and `serde_json` (streaming serialization via `to_writer`). `serde` needs its `rc` feature enabled: `CellValue::Text`'s `Arc<str>` field only gets a `Serialize` impl with that feature on (found at implementation time — without it, `Arc<str>` doesn't implement `Serialize` at all, since serde gates `Rc`/`Arc` support behind `rc` to avoid the footgun of silently duplicating shared data across independent serializations).
- Depended on by: `lib.rs` (calls it explicitly on a `Workbook` — see [pipeline.md Open Question 1](pipeline.en.md); `pipeline.rs`'s `run` itself never calls it)

`JsonWorkbook` / `SheetSeq` / `JsonSheet` / `CellSeq` each hold only a borrow of the model (`&'a Workbook` / `&'a Sheet`), never owning a value. Their `Serialize` impls only walk the model once actually invoked, which naturally lines up with the sequential calls `serde_json::to_writer` makes internally — no intermediate data structure representing a whole sheet or the whole book is ever built on the heap.

`model::Workbook` / `Sheet` / `Cell` are deliberately not annotated with `#[derive(Serialize)]` directly; instead this file converts them into its own borrowing wrapper types before serializing. The reasoning mirrors [error.md](error.en.md)'s design decision to type-erase `Error::XmlParse::source` so `quick-xml` never becomes a public dependency: pulling a `serde` dependency into `model/` would let `serde`'s breaking changes ripple into `model/`'s own type definitions. Interposing this wrapper layer keeps `model/` as architecture.md's policy intends — "a pure data structure with no dependency on XML parsing or resolution logic" — and lets the JSON output's concrete field names and shape evolve independently of `model/`'s type definitions.

## Error Handling Policy

- `to_json_writer` / `to_json_string` return `Result<_, Error>`. `serde_json::to_writer` is specified to return `Err` when it encounters a non-finite float (`NaN`/`Infinity`), but this file always converts a non-finite `f64` into `JsonCellValue::Empty` inside `cell_value_to_json` before it ever reaches `serde_json`, so in practice no error arises from that path. `Result` is still returned regardless, for two reasons: (1) if `writer` is a `File`, network socket, or other I/O-backed implementation, the write itself can genuinely fail; (2) to uphold the existing policy of never using `unwrap`/`expect` internally ([error.md Error Handling Policy](error.en.md))
- `serde_json::Error` is never placed directly as a concrete type on an `Error` field; it is wrapped, type-erased as `Box<dyn std::error::Error + Send + Sync + 'static>`, in the newly added `Error::JsonSerialize` variant — the same reasoning as `error.md`'s `XmlParse::source`, keeping `serde_json` out of the public dependency surface
- The one value JSON cannot represent — `f64`'s `NaN`/`Infinity` (which `CellValue::Number` can in theory hold) — does not cause the whole document's serialization to be abandoned via `Err`; instead, only that cell falls back to `JsonCellValue::Empty` (equivalent to JSON `null`), and processing continues. It is not silently substituted with a valid number like `0.0`, because in the accounting/business-system use case requirements chapter 1 targets, that would make "a failed or undefined computation" indistinguishable from "a value that legitimately evaluated to zero" for any downstream aggregation (reflects the [PR #10 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/10#pullrequestreview-4949223332))

## Testing Strategy

- Verify that a `Workbook` with a single sheet and a single (numeric) cell converts to the expected JSON string via `to_json_string`
- Verify that for a sheet with a merged cell, the origin cell's `rowSpan`/`colSpan` are computed correctly and the virtual cell coordinates are never included in the `cells` array (wiring to `Sheet::iter_cells`'s origin-only design)
- Verify that for an unmerged, ordinary cell, the `rowSpan`/`colSpan` fields are omitted from the serialized output (`skip_serializing_if`)
- Verify that each `CellValue` variant (`Number`/`Text`/`Boolean`/`Error`) serializes correctly with the corresponding `type` tag and `value`
- Verify that `value: None` (a formatting-only cell) serializes as `type: "empty"`
- **Verify that a cell holding `CellValue::Number(f64::NAN)` / `CellValue::Number(f64::INFINITY)` never returns `Err` and instead outputs `type: "empty"`** (a regression test for the fallback specification added by the PR #10 review — explicitly distinguishing it from the previous `0.0` fallback)
- Verify that for a `Workbook` containing `Hidden`/`VeryHidden` sheets, every sheet appears in the output with its `visibility` field (wiring to [model/workbook.md](model/workbook.en.md)'s "include every sheet regardless of visibility" policy)
- Verify that a `Workbook` with zero sheets serializes correctly as `{"sheets": []}`
- **A regression test verifying that calling `to_json_writer` on a sheet with many cells does not cause additional heap allocation to grow significantly beyond `Sheet`'s own memory footprint (i.e. `JsonCell`s are never collected into a `Vec`)** (a test substantiating the peak-memory design intent raised by the PR #10 review; the concrete verification method is to be settled at implementation time, together with the choice of memory-profiling tooling)
- Verify that `Error::JsonSerialize` propagates when `to_json_writer` is given a `Write` implementation (a test mock) that fails partway through writing
- **Verify that `Sheet::col_width_ranges`/`default_col_width` serialize as a sheet-level `columns` array / `defaultColumnWidth` field, and are never duplicated onto individual cell objects** (Issue #39; the "sheet-level array, not per-cell" design decision is the thing under test here — see model/sheet.en.md)
- **Verify that `Sheet::images` serializes as a sheet-level `images` array; a `TwoCell` anchor emits `{"type":"twoCell","from":...,"to":...}` and a `OneCell` one `{"type":"oneCell","from":...,"ext":...}`; and `hyperlink` is omitted (not `null`) when the image carries none** (Issue #65)
- **Verify a styled cell's `font` (`size_pt`/`bold`) serializes nested under a per-cell `style` object, and that an unstyled cell (`Cell.style: None`) omits the `style` field entirely** (Issue #38 — the opposite sparseness decision from `columns`, since font genuinely varies cell-to-cell)
- **Verify `style.wrapText` serializes alongside `style.font` under the same per-cell `style` object, for both `true` and `false`** (Issue #37 — reuses the same styled/unstyled sparseness wiring `font` already established, since `JsonStyle` now always carries both fields together)
- **Verify a `CellValue::DateTime` cell serializes as `{"type": "dateTime", "value": "..."}` with an ISO 8601 string, and that single-digit calendar fields are zero-padded** (e.g. `2024-01-05T03:05:09`, not `2024-1-5T3:5:9` — Issue #40)
- **Verify a styled cell with a resolved `number_format` serializes `style.numberFormat` as that string, and that a styled cell with `number_format: None` ("General") omits the field entirely even though `style` itself is present** (Issue #41 — the opposite sparseness decision from `font`/`wrap_text` within the same already-present `style` object, since "General" carries no information a downstream consumer needs)
- **Verify `style.alignment` is always present (never omitted) and serializes each `Alignment` variant as the matching camelCase string, including `"general"` for the default** (Issue #42 — the same "always present" sparseness decision as `font`/`wrap_text`, not `numberFormat`)
- **Verify a styled cell with `ColorRef::Rgb`/`Theme`/`Indexed` serializes `style.fillFgColor`/`fillBgColor` tagged the same way `JsonCellValue` is (e.g. `{"type":"rgb","value":"FFFF0000"}`), that a `Theme` with no `tint` serializes `tint` as JSON `null` (not omitted — only the outer `fillFgColor`/`fillBgColor` key itself is ever omitted), and that a cell with no fill color omits both fields entirely** (Issue #75 — the same "opposite sparseness decision from `font`/`wrap_text`" `numberFormat` already established)
- **Verify a cell with `Borders { top: true, ... }` (any side `true`) serializes `style.borders` as `{"top":true,"right":false,"bottom":false,"left":false}` (all four keys present together, `false` sides not individually omitted), and that a cell with `Borders::default()` (no side at all) omits the `borders` key from `style` entirely** (Issue #97)

## Open Questions

1. ~~Whether to tag value kinds in the JSON structure~~ → **Resolved**: keep the tagged representation, `{"type": "number", "value": 42}` (reflects the [PR #10 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/10#pullrequestreview-4949223332)). Dropping the tag in favor of native JSON types alone would leave the frontend unable to distinguish `dateTime` from a plain string (`Text`), forcing string parsing wherever a date picker or formatting needs to apply; it would also remove the ability to distinguish `error` (a formula error value) from an ordinary string for grid warning styling; and it would prevent a type-safe TypeScript client built on a Discriminated Union keyed by `type`.
2. ~~Fallback value for non-finite floating-point numbers (`NaN`/`Infinity`)~~ → **Resolved**: falls back to `JsonCellValue::Empty` (equivalent to `null`) rather than `0.0` (reflects the [PR #10 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/10#pullrequestreview-4949223332)). See Error Handling Policy for details.
3. ~~`DateTime`'s string representation format~~ → **Resolved** (Issue #40): ISO 8601 without a timezone designator or fractional seconds, e.g. `"2024-01-01T13:45:30"`. A date-only cell serializes with a midnight time component (`T00:00:00`) rather than omitting the time — Excel itself doesn't distinguish date-only from date+time as a type, so there is no extra information to report either way, and a uniform shape is simpler for a downstream consumer to parse than a format that varies cell-to-cell. `format_date_time` now reads `DateTimeValue`'s real `year`/`month`/`day`/`hour`/`minute`/`second` fields (see [model/cell.md Open Question 4](model/cell.en.md), also resolved by Issue #40) directly into this format.
4. **JSON output of style information**: further resolved — `JsonCell.style.font` (Issue #38), `JsonCell.style.wrapText` (Issue #37), `JsonCell.style.numberFormat` (Issue #41), `JsonCell.style.alignment` (Issue #42), `JsonCell.style.fillFgColor`/`fillBgColor` (Issue #75), and `JsonCell.style.borders` (Issue #97) are all implemented as described above. Every sub-issue tracked at [model/style.md Open Question 1](model/style.en.md) is now resolved, plus the follow-on fill-color and border issues. `fillFgColor`/`fillBgColor` are kept raw/unresolved (tagged `rgb`/`theme`/`indexed`, not a final displayed color) — resolving them to an actual RGB value for rendering is Issue #76, out of this file's scope. `borders` reports presence only (not line style/weight/color), matching `model::style::Borders`'s own scope.
5. ~~Peak memory from batch construction~~ → **Resolved**: switched to a streaming design that never pre-builds a `Vec<JsonCell>` — the iterator from `Sheet::iter_cells` is fed directly into `serde::ser::SerializeSeq` inside `CellSeq::serialize` (reflects the [PR #10 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/10#pullrequestreview-4949223332)). Note that `to_json_string` (the convenience version backed internally by a `Vec<u8>` buffer) still requires O(n) memory proportional to the output size; a caller that truly wants O(1) additional memory needs to pass `to_json_writer` a real I/O destination such as a `BufWriter<File>`. Also, since `Sheet::iter_cells` makes no `ExactSizeIterator` guarantee, `serialize_seq`'s element-count hint is passed as `None` (this doesn't affect the JSON output's correctness, but forgoes a minor optimization opportunity some serializer implementations could otherwise take) — whether [model/sheet.md](model/sheet.en.md) should commit to `ExactSizeIterator` as part of its public API remains an open consideration there.
6. **Relationship between `to_json_writer`/`to_json_string` and `lib.rs`'s public API**: how `lib.rs` exposes these functions separately from `parse_workbook` (which returns `Workbook`) is tied to [pipeline.md Open Question 1](pipeline.en.md) and is to be settled when `lib.rs` is designed.
