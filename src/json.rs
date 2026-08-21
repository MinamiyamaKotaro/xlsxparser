// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Phase 5: serializes a fully resolved `model::Workbook` to JSON,
//! streaming cell-by-cell rather than buffering a whole sheet in memory.

use crate::error::Error;
use crate::model::{
    Alignment, AnchorMarker, Borders, Cell, CellRef, CellValue, ColWidthRange, ColorRef,
    DateTimeValue, Hyperlink, Image, ImageAnchor, Sheet, SheetVisibility, Workbook,
};
use serde::ser::{SerializeSeq, SerializeStruct};
use serde::{Serialize, Serializer};
use std::io::Write;

/// Streams `workbook` out as JSON to `writer`. Each element of the `cells`
/// array is converted from `Sheet::iter_cells` and written out one at a
/// time, without buffering a whole sheet's `Vec<JsonCell>` in memory. If
/// `writer` is, say, a `BufWriter<File>`, additional memory usage stays at
/// O(1) — one cell's worth at a time.
pub fn to_json_writer<W: Write>(workbook: &Workbook, writer: W) -> Result<(), Error> {
    let json_workbook = JsonWorkbook { workbook };
    serde_json::to_writer(writer, &json_workbook).map_err(|source| Error::JsonSerialize {
        source: Box::new(source),
    })
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
    // of never using unwrap/expect internally, it is still handled as a
    // Result.
    String::from_utf8(buf).map_err(|source| Error::JsonSerialize {
        source: Box::new(source),
    })
}

/// A borrowing wrapper over `model::Workbook`. Owns no value; its
/// `Serialize` impl walks the model on demand, achieving streaming.
struct JsonWorkbook<'a> {
    workbook: &'a Workbook,
}

impl Serialize for JsonWorkbook<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Workbook", 1)?;
        state.serialize_field(
            "sheets",
            &SheetSeq {
                workbook: self.workbook,
            },
        )?;
        state.end()
    }
}

struct SheetSeq<'a> {
    workbook: &'a Workbook,
}

impl Serialize for SheetSeq<'_> {
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

impl Serialize for JsonSheet<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Sheet", 8)?;
        state.serialize_field("name", &self.sheet.name)?;
        state.serialize_field("visibility", visibility_tag(self.sheet.visibility))?;
        state.serialize_field("maxRow", &self.sheet.max_row)?;
        state.serialize_field("maxCol", &self.sheet.max_col)?;
        state.serialize_field("defaultColumnWidth", &self.sheet.default_col_width())?;
        state.serialize_field("columns", &ColumnSeq { sheet: self.sheet })?;
        state.serialize_field("cells", &CellSeq { sheet: self.sheet })?;
        state.serialize_field("images", &ImageSeq { sheet: self.sheet })?;
        state.end()
    }
}

/// Emits `Sheet::col_width_ranges` as a sheet-level array
/// (`[{"min":1,"max":5,"width":10.0}, ...]`) rather than duplicating a
/// `columnWidth` value onto every cell: a column-level property repeated
/// once per populated cell in that column would multiply output size for
/// no benefit, undermining the sparse-output design this library exists
/// for (Issue #36 review discussion).
struct ColumnSeq<'a> {
    sheet: &'a Sheet,
}

impl Serialize for ColumnSeq<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let ranges = self.sheet.col_width_ranges();
        let mut seq = serializer.serialize_seq(Some(ranges.len()))?;
        for range in ranges {
            seq.serialize_element(&JsonColumn(range))?;
        }
        seq.end()
    }
}

struct JsonColumn<'a>(&'a ColWidthRange);

impl Serialize for JsonColumn<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Column", 3)?;
        state.serialize_field("min", &self.0.min)?;
        state.serialize_field("max", &self.0.max)?;
        state.serialize_field("width", &self.0.width)?;
        state.end()
    }
}

/// Converts each cell from `Sheet::iter_cells` into a `JsonCell` and writes
/// it straight to the serializer, one at a time, without ever building an
/// intermediate `Vec<JsonCell>`.
struct CellSeq<'a> {
    sheet: &'a Sheet,
}

impl Serialize for CellSeq<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Sheet::iter_cells is exposed only as `impl Iterator` and makes no
        // size-hint guarantee, so `None` is used here.
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
    /// Omitted entirely when the cell carries no style at all
    /// (`Cell.style: None`) — a plain-formatted cell doesn't pay for an
    /// empty `"style": {}` object. Per-cell, unlike `columns` (Issue #36
    /// review discussion): font genuinely varies cell-to-cell within the
    /// same column, so there is no sparse-output principle to preserve by
    /// hoisting it elsewhere the way `columnWidth` was (see
    /// docs/design/model/sheet.en.md's "Feature: column width" note).
    #[serde(skip_serializing_if = "Option::is_none")]
    style: Option<JsonStyle>,
    /// Omitted entirely when the cell carries no hyperlink at all (Issue
    /// #95) — same "no empty placeholder" convention as `style`.
    #[serde(skip_serializing_if = "Option::is_none")]
    hyperlink: Option<JsonHyperlink>,
}

fn is_one(n: &u32) -> bool {
    *n == 1
}

/// `model::Hyperlink`'s JSON form (Issue #95). Every field mirrors the
/// model 1:1, kept in the same raw/unresolved form — see `Hyperlink`'s own
/// doc comment for why (Issue #75's `ColorRef` precedent).
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct JsonHyperlink {
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tooltip: Option<String>,
}

fn hyperlink_to_json(h: &Hyperlink) -> JsonHyperlink {
    JsonHyperlink {
        target: h.target.clone(),
        location: h.location.clone(),
        tooltip: h.tooltip.clone(),
    }
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
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct JsonBorders {
    top: bool,
    right: bool,
    bottom: bool,
    left: bool,
}

fn borders_to_json(b: &Borders) -> Option<JsonBorders> {
    b.any().then_some(JsonBorders {
        top: b.top,
        right: b.right,
        bottom: b.bottom,
        left: b.left,
    })
}

/// `ColorRef`'s JSON form (Issue #75), tagged the same way `JsonCellValue`
/// is (`#[serde(tag = "type", content = "value")]`) for consistency across
/// this module's every kind-tagged value — e.g. `{"type":"theme","value":
/// {"index":4,"tint":-0.25}}`. Kept in its raw/unresolved form, same as
/// `model::ColorRef` itself (see that type's doc comment for why).
#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
enum JsonColorRef {
    Rgb(String),
    Theme { index: u32, tint: Option<f64> },
    Indexed(u32),
}

fn color_ref_to_json(c: &ColorRef) -> JsonColorRef {
    match c {
        ColorRef::Rgb(s) => JsonColorRef::Rgb(s.to_string()),
        ColorRef::Theme { index, tint } => JsonColorRef::Theme {
            index: *index,
            tint: *tint,
        },
        ColorRef::Indexed(i) => JsonColorRef::Indexed(*i),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonFont {
    size_pt: f64,
    bold: bool,
}

/// A kind-tagged value representation. `#[serde(tag = "type", content =
/// "value")]` serializes as `{"type": "number", "value": 42.0}`.
#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
enum JsonCellValue {
    Number(f64),
    /// ISO 8601, no fractional seconds (`DateTimeValue` doesn't carry
    /// sub-second precision — see `model/cell.rs`), e.g.
    /// `"2024-01-01T13:45:30"`. A date-only cell serializes with a midnight
    /// time component, since Excel itself doesn't distinguish date-only
    /// from date+time as a type — there is no extra information to report.
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
            font: JsonFont {
                size_pt: s.font.size_pt,
                bold: s.font.bold,
            },
            wrap_text: s.wrap_text,
            alignment: alignment_tag(s.horizontal_alignment),
            number_format: s.number_format.as_deref().map(str::to_string),
            fill_fg_color: s.fill_fg_color.as_ref().map(color_ref_to_json),
            fill_bg_color: s.fill_bg_color.as_ref().map(color_ref_to_json),
            borders: borders_to_json(&s.borders),
        }),
        hyperlink: sheet.hyperlink_at(cell_ref).map(hyperlink_to_json),
    }
}

fn cell_value_to_json(value: Option<&CellValue>) -> JsonCellValue {
    match value {
        None => JsonCellValue::Empty,
        Some(CellValue::Number(n)) if n.is_finite() => JsonCellValue::Number(*n),
        // Silently substituting 0.0 for NaN/Infinity would make it
        // indistinguishable, downstream, from a value that legitimately
        // evaluated to zero, risking incorrect aggregation results (given
        // the accounting/business-system use case). Falling back to Empty
        // (JSON `null`) instead lets the frontend safely treat it as "no
        // value present."
        Some(CellValue::Number(_)) => JsonCellValue::Empty,
        Some(CellValue::DateTime(dt)) => JsonCellValue::DateTime(format_date_time(dt)),
        Some(CellValue::Text(s)) => JsonCellValue::Text(s.clone()),
        Some(CellValue::Boolean(b)) => JsonCellValue::Boolean(*b),
        Some(CellValue::Error(e)) => JsonCellValue::Error(e.clone()),
    }
}

/// Formats a `DateTimeValue` as ISO 8601 without a timezone designator or
/// fractional seconds, e.g. `"2024-01-01T13:45:30"`.
fn format_date_time(dt: &DateTimeValue) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
    )
}

/// `model::style::Alignment` doesn't derive `Serialize` directly (keeping
/// `serde` out of `model/`'s dependency surface — see this file's
/// Dependencies doc), so this mirrors `visibility_tag`'s pattern instead.
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

/// Emits `Sheet::images` as a sheet-level array, for the same reason
/// `ColumnSeq` does: an image isn't owned by any one cell (its anchor
/// markers don't always align to a cell boundary), so there is no cell to
/// attach it to even if duplication were otherwise desirable (Issue #65).
struct ImageSeq<'a> {
    sheet: &'a Sheet,
}

impl Serialize for ImageSeq<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let images = self.sheet.images();
        let mut seq = serializer.serialize_seq(Some(images.len()))?;
        for image in images {
            seq.serialize_element(&image_to_json(image))?;
        }
        seq.end()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonImage {
    anchor: JsonAnchor,
    target: String,
    /// Omitted entirely when the image carries no hyperlink of its own —
    /// same "no empty placeholder" convention as `JsonCell::style`.
    #[serde(skip_serializing_if = "Option::is_none")]
    hyperlink: Option<String>,
}

/// Internally tagged (`{"type": "twoCell", "from": ..., "to": ...}` /
/// `{"type": "oneCell", "from": ..., "ext": ...}`) rather than the
/// `{"type": ..., "value": ...}` shape `JsonCellValue` uses: each variant
/// here has more than one field of its own, so there is no single `value`
/// to nest them under.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum JsonAnchor {
    TwoCell { from: JsonMarker, to: JsonMarker },
    OneCell { from: JsonMarker, ext: JsonExtent },
}

/// A `<xdr:from>`/`<xdr:to>` marker. `row`/`col` are 1-based, matching every
/// other cell coordinate this crate emits (`JsonCell::row`/`col`) — the
/// 0-based `xdr:col`/`xdr:row` from the source XML is already converted by
/// `parse::drawing::zero_based_to_cell_ref` before this type ever sees it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonMarker {
    row: u32,
    col: u32,
    col_off: i64,
    row_off: i64,
}

/// `xdr:ext`: an image's width/height in EMU (English Metric Units),
/// present only on a `oneCell` anchor.
#[derive(Debug, Serialize)]
struct JsonExtent {
    cx: i64,
    cy: i64,
}

fn image_to_json(image: &Image) -> JsonImage {
    JsonImage {
        anchor: anchor_to_json(&image.anchor),
        target: image.target.clone(),
        hyperlink: image.hyperlink.clone(),
    }
}

fn anchor_to_json(anchor: &ImageAnchor) -> JsonAnchor {
    match anchor {
        ImageAnchor::TwoCell { from, to } => JsonAnchor::TwoCell {
            from: marker_to_json(from),
            to: marker_to_json(to),
        },
        ImageAnchor::OneCell { from, ext } => JsonAnchor::OneCell {
            from: marker_to_json(from),
            ext: JsonExtent {
                cx: ext.cx,
                cy: ext.cy,
            },
        },
    }
}

fn marker_to_json(marker: &AnchorMarker) -> JsonMarker {
    JsonMarker {
        row: marker.cell.row,
        col: marker.cell.col,
        col_off: marker.col_off,
        row_off: marker.row_off,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Font, ResolvedStyle};
    use std::sync::Arc;

    fn sheet_with_one_cell(name: &str, value: Option<CellValue>) -> Sheet {
        let mut sheet = Sheet::new(name.to_string(), SheetVisibility::Visible);
        sheet.insert_cell(CellRef { row: 1, col: 1 }, Cell { value, style: None });
        sheet
    }

    #[test]
    fn single_numeric_cell_round_trips_through_json_string() {
        let sheet = sheet_with_one_cell("Sheet1", Some(CellValue::Number(42.0)));
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed,
            serde_json::json!({
                "sheets": [{
                    "name": "Sheet1",
                    "visibility": "visible",
                    "maxRow": 1,
                    "maxCol": 1,
                    "defaultColumnWidth": null,
                    "columns": [],
                    "cells": [
                        {"row": 1, "col": 1, "value": {"type": "number", "value": 42.0}}
                    ],
                    "images": []
                }]
            })
        );
    }

    #[test]
    fn styled_cell_reports_font_and_wrap_text_nested_under_style() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(CellValue::Number(1.0)),
                style: Some(Arc::new(ResolvedStyle {
                    is_date_time: false,
                    font: Font {
                        size_pt: 14.0,
                        bold: true,
                    },
                    wrap_text: true,
                    ..Default::default()
                })),
            },
        );
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cell_json = &parsed["sheets"][0]["cells"][0];

        assert_eq!(
            cell_json["style"],
            serde_json::json!({
                "font": { "sizePt": 14.0, "bold": true },
                "wrapText": true,
                "alignment": "general"
            })
        );
    }

    #[test]
    fn styled_cell_with_number_format_reports_it_nested_under_style() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(CellValue::Number(0.5)),
                style: Some(Arc::new(ResolvedStyle {
                    number_format: Some(Arc::from("0%")),
                    ..Default::default()
                })),
            },
        );
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cell_json = &parsed["sheets"][0]["cells"][0];

        assert_eq!(cell_json["style"]["numberFormat"], "0%");
    }

    #[test]
    fn styled_cell_without_number_format_omits_the_field_entirely() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(CellValue::Number(1.0)),
                style: Some(Arc::new(ResolvedStyle::default())),
            },
        );
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cell_json = &parsed["sheets"][0]["cells"][0];

        assert!(cell_json["style"].get("numberFormat").is_none());
    }

    #[test]
    fn styled_cell_with_rgb_fill_reports_it_tagged_under_style() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(CellValue::Number(1.0)),
                style: Some(Arc::new(ResolvedStyle {
                    fill_fg_color: Some(ColorRef::Rgb(Arc::from("FFFF0000"))),
                    fill_bg_color: Some(ColorRef::Rgb(Arc::from("FF00FF00"))),
                    ..Default::default()
                })),
            },
        );
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cell_json = &parsed["sheets"][0]["cells"][0];

        assert_eq!(
            cell_json["style"]["fillFgColor"],
            serde_json::json!({ "type": "rgb", "value": "FFFF0000" })
        );
        assert_eq!(
            cell_json["style"]["fillBgColor"],
            serde_json::json!({ "type": "rgb", "value": "FF00FF00" })
        );
    }

    #[test]
    fn styled_cell_with_theme_and_tint_fill_reports_both() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(CellValue::Number(1.0)),
                style: Some(Arc::new(ResolvedStyle {
                    fill_fg_color: Some(ColorRef::Theme {
                        index: 4,
                        tint: Some(-0.25),
                    }),
                    ..Default::default()
                })),
            },
        );
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cell_json = &parsed["sheets"][0]["cells"][0];

        assert_eq!(
            cell_json["style"]["fillFgColor"],
            serde_json::json!({ "type": "theme", "value": { "index": 4, "tint": -0.25 } })
        );
    }

    #[test]
    fn styled_cell_with_theme_fill_and_no_tint_reports_tint_as_null() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(CellValue::Number(1.0)),
                style: Some(Arc::new(ResolvedStyle {
                    fill_fg_color: Some(ColorRef::Theme {
                        index: 1,
                        tint: None,
                    }),
                    ..Default::default()
                })),
            },
        );
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cell_json = &parsed["sheets"][0]["cells"][0];

        assert_eq!(
            cell_json["style"]["fillFgColor"],
            serde_json::json!({ "type": "theme", "value": { "index": 1, "tint": null } })
        );
    }

    #[test]
    fn styled_cell_with_indexed_fill_reports_it() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(CellValue::Number(1.0)),
                style: Some(Arc::new(ResolvedStyle {
                    fill_fg_color: Some(ColorRef::Indexed(64)),
                    ..Default::default()
                })),
            },
        );
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cell_json = &parsed["sheets"][0]["cells"][0];

        assert_eq!(
            cell_json["style"]["fillFgColor"],
            serde_json::json!({ "type": "indexed", "value": 64 })
        );
    }

    #[test]
    fn styled_cell_without_fill_color_omits_both_fields_entirely() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(CellValue::Number(1.0)),
                style: Some(Arc::new(ResolvedStyle::default())),
            },
        );
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cell_json = &parsed["sheets"][0]["cells"][0];

        assert!(cell_json["style"].get("fillFgColor").is_none());
        assert!(cell_json["style"].get("fillBgColor").is_none());
    }

    #[test]
    fn styled_cell_with_borders_reports_all_four_sides_together() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(CellValue::Number(1.0)),
                style: Some(Arc::new(ResolvedStyle {
                    borders: crate::model::Borders {
                        top: true,
                        right: false,
                        bottom: false,
                        left: false,
                    },
                    ..Default::default()
                })),
            },
        );
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cell_json = &parsed["sheets"][0]["cells"][0];

        assert_eq!(
            cell_json["style"]["borders"],
            serde_json::json!({"top": true, "right": false, "bottom": false, "left": false})
        );
    }

    #[test]
    fn styled_cell_without_any_border_omits_the_field_entirely() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(CellValue::Number(1.0)),
                style: Some(Arc::new(ResolvedStyle::default())),
            },
        );
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cell_json = &parsed["sheets"][0]["cells"][0];

        assert!(cell_json["style"].get("borders").is_none());
    }

    #[test]
    fn cell_with_hyperlink_reports_it_nested_and_omits_absent_fields() {
        let mut sheet = sheet_with_one_cell("Sheet1", Some(CellValue::Number(1.0)));
        sheet.finalize_hyperlinks(vec![crate::model::HyperlinkRange {
            start: CellRef { row: 1, col: 1 },
            end: CellRef { row: 1, col: 1 },
            hyperlink: crate::model::Hyperlink {
                target: Some("https://example.com/".to_string()),
                location: None,
                tooltip: Some("Visit example".to_string()),
            },
        }]);
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cell_json = &parsed["sheets"][0]["cells"][0];

        assert_eq!(
            cell_json["hyperlink"],
            serde_json::json!({
                "target": "https://example.com/",
                "tooltip": "Visit example"
            })
        );
        assert!(cell_json["hyperlink"].get("location").is_none());
    }

    #[test]
    fn cell_without_hyperlink_omits_the_field_entirely() {
        let sheet = sheet_with_one_cell("Sheet1", Some(CellValue::Number(1.0)));
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cell_json = &parsed["sheets"][0]["cells"][0];

        assert!(cell_json.get("hyperlink").is_none());
    }

    #[test]
    fn alignment_tag_covers_every_variant() {
        assert_eq!(alignment_tag(Alignment::General), "general");
        assert_eq!(alignment_tag(Alignment::Left), "left");
        assert_eq!(alignment_tag(Alignment::Center), "center");
        assert_eq!(alignment_tag(Alignment::Right), "right");
        assert_eq!(alignment_tag(Alignment::Fill), "fill");
        assert_eq!(alignment_tag(Alignment::Justify), "justify");
        assert_eq!(
            alignment_tag(Alignment::CenterContinuous),
            "centerContinuous"
        );
        assert_eq!(alignment_tag(Alignment::Distributed), "distributed");
    }

    #[test]
    fn unstyled_cell_omits_the_style_field_entirely() {
        let sheet = sheet_with_one_cell("Sheet1", Some(CellValue::Number(1.0)));
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cell_json = &parsed["sheets"][0]["cells"][0];

        assert!(cell_json.get("style").is_none());
    }

    #[test]
    fn column_widths_serialize_as_a_sheet_level_array_not_per_cell() {
        let mut sheet = sheet_with_one_cell("Sheet1", Some(CellValue::Number(1.0)));
        sheet.set_col_widths(
            vec![
                ColWidthRange {
                    min: 1,
                    max: 5,
                    width: 12.0,
                },
                ColWidthRange {
                    min: 10,
                    max: 20,
                    width: 20.0,
                },
            ],
            Some(8.43),
        );
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let sheet_json = &parsed["sheets"][0];

        assert_eq!(sheet_json["defaultColumnWidth"], serde_json::json!(8.43));
        assert_eq!(
            sheet_json["columns"],
            serde_json::json!([
                {"min": 1, "max": 5, "width": 12.0},
                {"min": 10, "max": 20, "width": 20.0}
            ])
        );
        // The columns array is sheet-level, not duplicated onto the cell.
        assert!(sheet_json["cells"][0].get("columnWidth").is_none());
    }

    #[test]
    fn merged_cell_reports_span_and_excludes_virtual_coordinates() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(CellValue::Text(Arc::from("merged"))),
                style: None,
            },
        );
        sheet.insert_merge(crate::model::MergedRegion {
            start: CellRef { row: 1, col: 1 },
            end: CellRef { row: 2, col: 3 },
        });
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cells = parsed["sheets"][0]["cells"].as_array().unwrap();
        assert_eq!(cells.len(), 1, "virtual coordinates must not appear");
        assert_eq!(cells[0]["rowSpan"], 2);
        assert_eq!(cells[0]["colSpan"], 3);
    }

    #[test]
    fn unmerged_cell_omits_span_fields() {
        let sheet = sheet_with_one_cell("Sheet1", Some(CellValue::Number(1.0)));
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cell = &parsed["sheets"][0]["cells"][0];
        assert!(cell.get("rowSpan").is_none());
        assert!(cell.get("colSpan").is_none());
    }

    #[test]
    fn each_cell_value_variant_serializes_with_expected_tag() {
        assert_eq!(
            cell_value_to_json(Some(&CellValue::Number(1.5))),
            JsonCellValue::Number(1.5)
        );
        assert_eq!(
            cell_value_to_json(Some(&CellValue::Text(Arc::from("hi")))),
            JsonCellValue::Text(Arc::from("hi"))
        );
        assert_eq!(
            cell_value_to_json(Some(&CellValue::Boolean(true))),
            JsonCellValue::Boolean(true)
        );
        assert_eq!(
            cell_value_to_json(Some(&CellValue::Error("#DIV/0!".into()))),
            JsonCellValue::Error("#DIV/0!".into())
        );
    }

    #[test]
    fn formatting_only_cell_serializes_as_empty() {
        let style = Arc::new(ResolvedStyle {
            is_date_time: false,
            ..Default::default()
        });
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: None,
                style: Some(style),
            },
        );
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["sheets"][0]["cells"][0]["value"]["type"], "empty");
    }

    #[test]
    fn non_finite_numbers_fall_back_to_empty_without_erroring() {
        for n in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let sheet = sheet_with_one_cell("Sheet1", Some(CellValue::Number(n)));
            let workbook = Workbook::new(vec![sheet], None);

            let json = to_json_string(&workbook).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(
                parsed["sheets"][0]["cells"][0]["value"]["type"], "empty",
                "n = {n}"
            );
        }
    }

    #[test]
    fn date_time_value_serializes_as_iso_8601_without_fractional_seconds() {
        let dt = DateTimeValue {
            year: 2024,
            month: 1,
            day: 5,
            hour: 13,
            minute: 45,
            second: 30,
        };
        assert_eq!(
            cell_value_to_json(Some(&CellValue::DateTime(dt))),
            JsonCellValue::DateTime("2024-01-05T13:45:30".into())
        );

        let json =
            serde_json::to_string(&JsonCellValue::DateTime("2024-01-01T00:00:00".into())).unwrap();
        assert_eq!(json, r#"{"type":"dateTime","value":"2024-01-01T00:00:00"}"#);
    }

    #[test]
    fn date_time_value_pads_single_digit_calendar_fields() {
        // Regression test for the `{:02}`/`{:04}` format specifiers in
        // `format_date_time` — a naive `{}` would produce "2024-1-5T3:5:9",
        // which is not valid ISO 8601.
        let dt = DateTimeValue {
            year: 2024,
            month: 1,
            day: 5,
            hour: 3,
            minute: 5,
            second: 9,
        };
        assert_eq!(format_date_time(&dt), "2024-01-05T03:05:09");
    }

    #[test]
    fn styled_date_time_cell_round_trips_through_json_string() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(CellValue::DateTime(DateTimeValue {
                    year: 2023,
                    month: 3,
                    day: 15,
                    hour: 0,
                    minute: 0,
                    second: 0,
                })),
                style: None,
            },
        );
        let workbook = Workbook::new(vec![sheet], None);

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed["sheets"][0]["cells"][0]["value"],
            serde_json::json!({"type": "dateTime", "value": "2023-03-15T00:00:00"})
        );
    }

    #[test]
    fn hidden_and_very_hidden_sheets_report_visibility() {
        let workbook = Workbook::new(
            vec![
                Sheet::new("Vis".into(), SheetVisibility::Visible),
                Sheet::new("Hid".into(), SheetVisibility::Hidden),
                Sheet::new("VHid".into(), SheetVisibility::VeryHidden),
            ],
            None,
        );

        let json = to_json_string(&workbook).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let visibilities: Vec<&str> = parsed["sheets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["visibility"].as_str().unwrap())
            .collect();
        assert_eq!(visibilities, vec!["visible", "hidden", "veryHidden"]);
    }

    #[test]
    fn zero_sheet_workbook_serializes_as_empty_array() {
        let workbook = Workbook::new(vec![], None);
        assert_eq!(to_json_string(&workbook).unwrap(), r#"{"sheets":[]}"#);
    }

    #[test]
    fn failing_writer_propagates_as_json_serialize_error() {
        struct FailingWriter;
        impl Write for FailingWriter {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("write failed"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let workbook = Workbook::new(
            vec![sheet_with_one_cell("Sheet1", Some(CellValue::Number(1.0)))],
            None,
        );
        let err = to_json_writer(&workbook, FailingWriter).unwrap_err();
        assert!(matches!(err, Error::JsonSerialize { .. }));
    }
}
