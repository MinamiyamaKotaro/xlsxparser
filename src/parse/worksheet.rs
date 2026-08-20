// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Phase 3: SAX-style streaming parse of `xl/worksheets/sheetX.xml`.
//! Streams `<sheetData>` row by row, inserting cells into `Sheet` directly,
//! while collecting the pieces that need Phase 4's deferred resolution
//! (shared strings, styles, merged ranges).

use crate::error::Error;
use crate::model::{
    Cell, CellRef, CellValue, ColWidthRange, DateTimeValue, MergedRegion, Sheet, StyleId,
};
use crate::parse::{
    concat_rich_text, create_secure_reader, optional_attr, read_event, read_leaf_text,
    required_attr,
};
use quick_xml::events::Event;
use std::io::BufRead;
use std::sync::Arc;

/// The pending entry Phase 3 records when it detects a `t="s"` cell.
/// `model::CellValue` only ever admits an already-resolved `Text(Arc<str>)`
/// and has no variant that holds a raw index, so at parse time the cell
/// itself is inserted into `Sheet` with `value: None`, and the index is
/// kept outside the sheet in this struct instead.
/// `resolve/shared_strings.rs` consumes this to resolve the actual string.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingSharedString {
    pub cell_ref: CellRef,
    pub index: usize,
}

/// The pending entry Phase 3 records when it detects a cell carrying an `s`
/// (style index) attribute. `resolve/style.rs` consumes this to apply the
/// `ResolvedStyle`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingStyle {
    pub cell_ref: CellRef,
    pub style_id: StyleId,
}

/// The pending entry Phase 3 records for a `<hyperlink>` element (Issue
/// #95). Only syntactic parsing happens here — `r_id` (if present) is
/// resolved to a raw Target string in `pipeline.rs`, against the same
/// `worksheet_rels` map Issue #65's image resolution already loads;
/// `start`/`end` ordering and mutual overlap with other hyperlink ranges
/// are validated by `resolve/hyperlink.rs`, not here — same division of
/// labor as `MergedRegion`/`resolve/merge.rs`.
///
/// `start`/`end` mirror a single coordinate (`ref="A1"`) as `start ==
/// end`, or the two ends of a range (`ref="A1:C3"`) otherwise —
/// `parse_hyperlink_ref` never expands a range into individual cells, for
/// the same O(row_span * col_span) amplification reason `MergedRegion`
/// isn't expanded either.
#[derive(Debug, Clone)]
pub(crate) struct PendingHyperlink {
    pub start: CellRef,
    pub end: CellRef,
    pub r_id: Option<String>,
    pub location: Option<String>,
    pub tooltip: Option<String>,
}

/// `parse_worksheet`'s output. `sheet` itself is mutated directly through
/// the `&mut` argument, so this only returns the three remaining pieces of
/// unresolved data Phase 4 needs.
#[derive(Debug)]
pub(crate) struct WorksheetParseOutput {
    pub pending_shared_strings: Vec<PendingSharedString>,
    pub pending_styles: Vec<PendingStyle>,
    pub col_width_ranges: Vec<ColWidthRange>,
    pub default_col_width: Option<f64>,
    pub merge_regions: Vec<MergedRegion>,
    /// The `r:id` from `<drawing r:id="rIdX"/>`, if this worksheet has one.
    /// `pipeline.rs` resolves it against `xl/worksheets/_rels/sheetN.xml.rels`
    /// to locate the `drawingN.xml` part (Issue #65); `None` means this
    /// sheet has no images at all, so no `_rels` lookup is attempted for it.
    pub drawing_r_id: Option<String>,
    /// Every `<hyperlink>` entry collected from `<hyperlinks>` (Issue #95).
    /// Empty means this sheet declared no cell hyperlinks at all, in which
    /// case `pipeline.rs` skips loading `_rels` for hyperlink resolution
    /// entirely (same "pay for what you use" pattern as the theme color
    /// gate).
    pub pending_hyperlinks: Vec<PendingHyperlink>,
}

/// Phase 3's entry function. `sheet` is received already constructed by
/// `pipeline.rs` with `name`/`visibility` set from `parse/workbook.rs`'s
/// result; cells are streamed into it.
///
/// Calling contract: when a `t="s"` cell is detected, `insert_cell`-ing a
/// `Cell` with `value: None` and recording the corresponding
/// `PendingSharedString` always happen together. When a cell carries an `s`
/// attribute, `insert_cell` and recording the corresponding `PendingStyle`
/// always happen together. `resolve/shared_strings.rs` and
/// `resolve/style.rs` both rely on `Sheet::get_mut` succeeding on the
/// assumption that this invariant holds.
///
/// Each `<c>`'s attributes/text state is freshly initialized when that
/// `<c>`'s start tag is seen and fully consumed (inserted or discarded) by
/// the time its end tag is processed, so no state survives from one cell —
/// or one row — into the next.
///
/// `date1904` is `workbook.xml`'s `<workbookPr date1904="1"/>` flag,
/// otherwise a purely Phase 4 (`resolve/style.rs`) concern — it reaches
/// Phase 3 solely so a `t="d"` time-only cell's placeholder date (no date
/// component exists in the ISO 8601 source text) can agree with what a
/// numeric time-only cell in the same book would resolve to. See
/// `parse_iso8601_datetime`'s doc comment (PR #80 review discussion).
pub(crate) fn parse_worksheet(
    reader: impl BufRead,
    path: &str,
    sheet: &mut Sheet,
    date1904: bool,
    max_cells: usize,
) -> Result<WorksheetParseOutput, Error> {
    let mut xml_reader = create_secure_reader(reader);
    let mut buf = Vec::new();
    let mut cell_count: usize = 0;
    let mut pending_shared_strings = Vec::new();
    let mut pending_styles = Vec::new();
    let mut col_width_ranges = Vec::new();
    let mut default_col_width = None;
    let mut merge_regions = Vec::new();
    let mut drawing_r_id = None;
    let mut pending_hyperlinks = Vec::new();

    // State for the `<c>` currently being read (between its start and end
    // tag). `cur_ref` doubles as "are we inside a <c>?".
    let mut cur_ref: Option<CellRef> = None;
    let mut cur_type: Option<String> = None;
    let mut cur_style: Option<u32> = None;
    let mut cur_value_text: Option<String> = None;
    let mut cur_inline: Option<String> = None;

    // Row/column position inference for `<row>`/`<c>` elements that omit
    // their optional `r` attribute (Issue #79 — ECMA-376 Part 1 §18.3.1.4/
    // §18.3.1.73 both mark `r` `use="optional"`, with the omitted position
    // inferred from context: a `<row>` without `r` is the row right after
    // the previous one, a `<c>` without `r` is the column right after the
    // previous cell in the same row). `cur_row == 0` doubles as "no `<row>`
    // seen yet".
    let mut cur_row: u32 = 0;
    let mut cur_col: u32 = 0;

    loop {
        let event = read_event(&mut xml_reader, &mut buf, path)?;
        match &event {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"row" => {
                cur_row = match optional_attr(e, path, "r")? {
                    Some(r) => parse_row_r_attr(&r, path)?,
                    None => cur_row + 1,
                };
                if cur_row == 0 || cur_row > CellRef::MAX_ROW {
                    return Err(Error::InvalidCellRef(format!("row {cur_row} in {path}")));
                }
                cur_col = 0;
            }
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"c" => {
                let is_empty = matches!(&event, Event::Empty(_));
                let cell_ref = match optional_attr(e, path, "r")? {
                    Some(r) => {
                        let cell_ref = CellRef::from_a1(&r)?;
                        cur_col = cell_ref.col;
                        cell_ref
                    }
                    None => {
                        cur_col += 1;
                        if cur_row == 0 || cur_col > CellRef::MAX_COL {
                            return Err(Error::InvalidCellRef(format!(
                                "row {cur_row}, col {cur_col} in {path}"
                            )));
                        }
                        CellRef {
                            row: cur_row,
                            col: cur_col,
                        }
                    }
                };
                let cell_type = optional_attr(e, path, "t")?;
                let style_id = optional_attr(e, path, "s")?.and_then(|s| s.parse::<u32>().ok());

                if is_empty {
                    let inserted = flush_cell(
                        sheet,
                        &mut pending_shared_strings,
                        &mut pending_styles,
                        path,
                        cell_ref,
                        cell_type,
                        style_id,
                        None,
                        None,
                        date1904,
                    )?;
                    if inserted {
                        check_cell_count(&mut cell_count, max_cells, path)?;
                    }
                } else {
                    cur_ref = Some(cell_ref);
                    cur_type = cell_type;
                    cur_style = style_id;
                    cur_value_text = None;
                    cur_inline = None;
                }
            }
            Event::Start(e) if cur_ref.is_some() && e.local_name().as_ref() == b"v" => {
                cur_value_text = Some(read_leaf_text(&mut xml_reader, path)?);
            }
            Event::Start(e) if cur_ref.is_some() && e.local_name().as_ref() == b"is" => {
                cur_inline = Some(concat_rich_text(&mut xml_reader, path)?);
            }
            Event::Start(e) if cur_ref.is_some() && e.local_name().as_ref() == b"f" => {
                // Formula text is neither parsed nor retained; only <v>'s
                // cached computed value is used.
                let _ = read_leaf_text(&mut xml_reader, path)?;
            }
            Event::End(e) if e.local_name().as_ref() == b"c" => {
                if let Some(cell_ref) = cur_ref.take() {
                    let inserted = flush_cell(
                        sheet,
                        &mut pending_shared_strings,
                        &mut pending_styles,
                        path,
                        cell_ref,
                        cur_type.take(),
                        cur_style.take(),
                        cur_value_text.take(),
                        cur_inline.take(),
                        date1904,
                    )?;
                    if inserted {
                        check_cell_count(&mut cell_count, max_cells, path)?;
                    }
                }
            }
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"mergeCell" => {
                let cell_range = required_attr(e, path, "ref")?;
                merge_regions.push(parse_merge_ref(&cell_range)?);
            }
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"sheetFormatPr" => {
                if let Some(w) = optional_attr(e, path, "defaultColWidth")? {
                    default_col_width = Some(parse_f64_attr(&w, "defaultColWidth", path)?);
                }
            }
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"col" => {
                if let Some(width_str) = optional_attr(e, path, "width")? {
                    let min = parse_u32_attr(&required_attr(e, path, "min")?, "min", path)?;
                    let max = parse_u32_attr(&required_attr(e, path, "max")?, "max", path)?;
                    let width = parse_f64_attr(&width_str, "width", path)?;
                    col_width_ranges.push(ColWidthRange { min, max, width });
                }
            }
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"drawing" => {
                drawing_r_id = Some(required_attr(e, path, "r:id")?);
            }
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"hyperlink" => {
                let cell_range = required_attr(e, path, "ref")?;
                let (start, end) = parse_hyperlink_ref(&cell_range, path)?;
                pending_hyperlinks.push(PendingHyperlink {
                    start,
                    end,
                    r_id: optional_attr(e, path, "r:id")?,
                    location: optional_attr(e, path, "location")?,
                    tooltip: optional_attr(e, path, "tooltip")?,
                });
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(WorksheetParseOutput {
        pending_shared_strings,
        pending_styles,
        col_width_ranges,
        default_col_width,
        merge_regions,
        drawing_r_id,
        pending_hyperlinks,
    })
}

/// Parses a `<row r="...">` attribute value as `u32`. Unlike
/// [`parse_u32_attr`] (used for `<col>`/`<sheetFormatPr>`), a malformed
/// value here is `Error::InvalidCellRef` rather than `Error::InvalidPackage`
/// — `r` is a coordinate, same as `<c r="...">`'s, so a non-numeric value
/// is reported the same way `CellRef::from_a1` reports one, rather than as
/// a generic package-level error (PR #81 review).
fn parse_row_r_attr(value: &str, path: &str) -> Result<u32, Error> {
    value
        .parse()
        .map_err(|_| Error::InvalidCellRef(format!("row r={value:?} in {path}")))
}

/// Parses a `<col>`/`<sheetFormatPr>` numeric attribute value as `u32`,
/// wrapping a malformed value as `Error::InvalidPackage` (same convention
/// as `read_leaf_text`'s numeric cell value parsing below).
fn parse_u32_attr(value: &str, attr_name: &str, path: &str) -> Result<u32, Error> {
    value
        .parse()
        .map_err(|_| Error::InvalidPackage(format!("invalid {attr_name} {value:?} in {path}")))
}

/// Same as [`parse_u32_attr`], but for `f64` (column width).
fn parse_f64_attr(value: &str, attr_name: &str, path: &str) -> Result<f64, Error> {
    value
        .parse()
        .map_err(|_| Error::InvalidPackage(format!("invalid {attr_name} {value:?} in {path}")))
}

/// Finalizes one `<c>`: decides whether it carries enough information to be
/// worth inserting at all (a sparse matrix never instantiates a fully blank
/// cell — no value, no style, not even a pending shared-string reference),
/// and if so, inserts it and records any deferred-resolution entries.
/// Returns whether a cell was actually inserted, so the caller can count it
/// against [`SizeLimits::max_cells_per_sheet`](crate::SizeLimits::max_cells_per_sheet)
/// — a dropped, information-free `<c>` costs nothing and must not count.
#[allow(clippy::too_many_arguments)]
fn flush_cell(
    sheet: &mut Sheet,
    pending_shared_strings: &mut Vec<PendingSharedString>,
    pending_styles: &mut Vec<PendingStyle>,
    path: &str,
    cell_ref: CellRef,
    cell_type: Option<String>,
    style_id: Option<u32>,
    value_text: Option<String>,
    inline_string: Option<String>,
    date1904: bool,
) -> Result<bool, Error> {
    let is_shared_string = cell_type.as_deref() == Some("s");
    let has_value = value_text.is_some() || inline_string.is_some();

    if style_id.is_none() && !has_value && !is_shared_string {
        return Ok(false);
    }

    let cell = build_cell(
        cell_type.as_deref(),
        value_text.as_deref(),
        inline_string,
        path,
        date1904,
    )?;
    sheet.insert_cell(cell_ref, cell);

    if is_shared_string {
        if let Some(text) = &value_text {
            let index: usize = text.trim().parse().map_err(|_| {
                Error::InvalidPackage(format!("invalid shared string index {text:?} in {path}"))
            })?;
            pending_shared_strings.push(PendingSharedString { cell_ref, index });
        }
    }
    if let Some(style_id) = style_id {
        pending_styles.push(PendingStyle { cell_ref, style_id });
    }

    Ok(true)
}

/// Increments `cell_count` for one just-inserted cell and rejects the whole
/// parse with `Error::TooManyCells` the moment it exceeds `max_cells`
/// (Issue #88) — called from both `flush_cell` call sites in
/// `parse_worksheet`'s main loop (`Event::Empty`/self-closing `<c>`, and
/// `Event::End` for a `<c>` with children), immediately after `flush_cell`
/// reports it actually inserted a cell. Checking incrementally here, rather
/// than after the whole sheet is collected, matters because the memory cost
/// this guards against accrues at the moment of insertion itself — by the
/// time a post-hoc count would run, the damage is already done.
fn check_cell_count(cell_count: &mut usize, max_cells: usize, path: &str) -> Result<(), Error> {
    *cell_count += 1;
    if *cell_count > max_cells {
        return Err(Error::TooManyCells {
            path: path.to_string(),
            count: *cell_count,
            limit: max_cells,
        });
    }
    Ok(())
}

/// Builds a `Cell` from the content of `<v>`/`<is>` based on `<c t="...">`'s
/// `t` attribute (absent implies Number). For `t="s"`, `value` is left
/// `None` — the caller ([`flush_cell`]) records the corresponding
/// `PendingSharedString` separately. `style` is always `None` here;
/// `resolve/style.rs` fills it in from the corresponding `PendingStyle`.
fn build_cell(
    cell_type: Option<&str>,
    value_text: Option<&str>,
    inline_string: Option<String>,
    path: &str,
    date1904: bool,
) -> Result<Cell, Error> {
    let value = match cell_type {
        None | Some("n") => value_text
            .map(|s| parse_number(s, path))
            .transpose()?
            .map(CellValue::Number),
        Some("s") => None,
        Some("str") => value_text.map(|s| CellValue::Text(Arc::from(s))),
        Some("inlineStr") => inline_string.map(|s| CellValue::Text(Arc::from(s))),
        Some("b") => value_text.map(|s| CellValue::Boolean(s == "1")),
        Some("e") => value_text.map(|s| CellValue::Error(s.to_string())),
        // ECMA-376 Part 1's `t="d"` extension (Issue #58): the `<v>` text is
        // an ISO 8601 string rather than a serial number.
        Some("d") => value_text
            .map(|s| parse_iso8601_datetime(s, path, date1904))
            .transpose()?
            .map(CellValue::DateTime),
        // Unknown `t` value: falls back to keeping the raw text as Text
        // rather than dropping data.
        Some(_) => value_text.map(|s| CellValue::Text(Arc::from(s))),
    };
    Ok(Cell { value, style: None })
}

fn parse_number(text: &str, path: &str) -> Result<f64, Error> {
    text.trim().parse::<f64>().map_err(|_| {
        Error::InvalidPackage(format!("invalid numeric cell value {text:?} in {path}"))
    })
}

/// Parses a `t="d"` cell's `<v>` text as ISO 8601 (Issue #58). Handles the
/// three shapes real files use: date-only (`2021-01-01`), date+time
/// (`2021-01-01T10:10:10`), and time-only (`10:10:10`), plus the following
/// tolerances a spec-compliant (if less common) writer may still emit (PR
/// #80 review):
/// - a trailing UTC/offset designator (`Z`, `+09:00`, `-0500`) is dropped —
///   `DateTimeValue` has no timezone field (mirroring Excel's own date
///   system, which isn't timezone-aware either), so the wall-clock value is
///   kept as-is rather than converted
/// - fractional seconds (`10:10:10.500`) are truncated to whole seconds
/// - seconds may be omitted entirely (`10:10`), defaulting to `:00`
///
/// A time-only value has no date component in the source, so it lands on
/// Excel's own "time of day" convention: serial day 0, which is 1899-12-30
/// under the default 1900 date system or 1904-01-01 under `date1904` —
/// matching how `resolve/style.rs::serial_to_date_time` already decodes a
/// fractional serial < 1 for numeric (non-ISO) cells in the same book. This
/// is the reason this otherwise Phase-4-flavored flag reaches Phase 3 at
/// all (see `parse_worksheet`'s doc comment).
fn parse_iso8601_datetime(text: &str, path: &str, date1904: bool) -> Result<DateTimeValue, Error> {
    let (date_part, time_part) = match text.split_once('T') {
        Some((d, t)) => (Some(d), Some(t)),
        None if text.contains(':') => (None, Some(text)),
        None => (Some(text), None),
    };

    let (year, month, day) = match date_part {
        Some(d) => {
            let segments: Vec<&str> = d.split('-').collect();
            let (y, mo, da) = match segments[..] {
                [y, mo, da] => (y, mo, da),
                _ => return Err(invalid_iso8601(text, path)),
            };
            let year = y.parse::<i32>().map_err(|_| invalid_iso8601(text, path))?;
            let month = mo.parse::<u8>().map_err(|_| invalid_iso8601(text, path))?;
            let day = da.parse::<u8>().map_err(|_| invalid_iso8601(text, path))?;
            (year, month, day)
        }
        // Excel's own "time of day with no date" convention: serial day 0,
        // whose calendar date itself depends on date1904 (matching
        // resolve/style.rs's EPOCH_OFFSET_1900/EPOCH_OFFSET_1904).
        None if date1904 => (1904, 1, 1),
        None => (1899, 12, 30),
    };

    let (hour, minute, second) = match time_part {
        Some(t) => {
            // Drop a trailing timezone designator before splitting on ':'
            // (see doc comment above) — restricted to this substring so a
            // date-only value's '-' separators are never mistaken for a
            // '-HH:MM' offset.
            let t = t.strip_suffix('Z').unwrap_or(t);
            let t = match t.rfind(['+', '-']) {
                Some(idx) if idx > 0 => &t[..idx],
                _ => t,
            };
            let segments: Vec<&str> = t.split(':').collect();
            let (h, mi, se) = match segments[..] {
                [h, mi, se] => (h, mi, se),
                [h, mi] => (h, mi, "0"),
                _ => return Err(invalid_iso8601(text, path)),
            };
            // Fractional seconds (e.g. "10.500") are truncated to whole
            // seconds rather than rejected.
            let se = se.split('.').next().unwrap_or(se);
            let hour = h.parse::<u8>().map_err(|_| invalid_iso8601(text, path))?;
            let minute = mi.parse::<u8>().map_err(|_| invalid_iso8601(text, path))?;
            let second = se.parse::<u8>().map_err(|_| invalid_iso8601(text, path))?;
            (hour, minute, second)
        }
        None => (0, 0, 0),
    };

    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(invalid_iso8601(text, path));
    }

    Ok(DateTimeValue {
        year,
        month,
        day,
        hour,
        minute,
        second,
    })
}

fn invalid_iso8601(text: &str, path: &str) -> Error {
    Error::InvalidPackage(format!("invalid ISO 8601 cell value {text:?} in {path}"))
}

/// Parses a `<mergeCell ref="A1:C3"/>` reference into a `MergedRegion`. Only
/// syntactic validity (two `:`-separated A1 coordinates) is checked here;
/// range soundness (start/end ordering, overlaps) is `resolve/merge.rs`'s
/// job.
fn parse_merge_ref(cell_range: &str) -> Result<MergedRegion, Error> {
    let (start_str, end_str) = cell_range
        .split_once(':')
        .ok_or_else(|| Error::InvalidCellRef(cell_range.to_string()))?;
    Ok(MergedRegion {
        start: CellRef::from_a1(start_str)?,
        end: CellRef::from_a1(end_str)?,
    })
}

/// Parses a `<hyperlink ref="...">` attribute (Issue #95) into a
/// `(start, end)` pair — either a plain coordinate (`"A1"`, the common
/// case; `start == end`) or a `"A1:C3"` range, kept as two coordinates
/// rather than expanded (see [`PendingHyperlink`]'s doc comment). Only
/// syntactic validity is checked here; range soundness (start/end
/// ordering, overlap with other hyperlink ranges) is
/// `resolve/hyperlink.rs`'s job — mirrors `parse_merge_ref` exactly.
fn parse_hyperlink_ref(cell_range: &str, path: &str) -> Result<(CellRef, CellRef), Error> {
    let invalid = || Error::InvalidCellRef(format!("hyperlink ref {cell_range:?} in {path}"));
    match cell_range.split_once(':') {
        Some((start_str, end_str)) => {
            let start = CellRef::from_a1(start_str).map_err(|_| invalid())?;
            let end = CellRef::from_a1(end_str).map_err(|_| invalid())?;
            Ok((start, end))
        }
        None => {
            let coord = CellRef::from_a1(cell_range).map_err(|_| invalid())?;
            Ok((coord, coord))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SheetVisibility;

    fn parse(xml: &[u8]) -> (Sheet, WorksheetParseOutput) {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let output = parse_worksheet(
            xml,
            "xl/worksheets/sheet1.xml",
            &mut sheet,
            false,
            usize::MAX,
        )
        .unwrap();
        (sheet, output)
    }

    #[test]
    fn parses_mixed_cell_types_in_one_row() {
        let xml = br##"<worksheet><sheetData><row r="1">
<c r="A1"><v>42</v></c>
<c r="B1" t="s"><v>0</v></c>
<c r="C1" t="b"><v>1</v></c>
<c r="D1" t="e"><v>#DIV/0!</v></c>
</row></sheetData></worksheet>"##;
        let (sheet, output) = parse(xml);

        assert_eq!(
            sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
            Some(CellValue::Number(42.0))
        );
        assert_eq!(sheet.get(CellRef { row: 1, col: 2 }).unwrap().value, None);
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 3 }).unwrap().value,
            Some(CellValue::Boolean(true))
        );
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 4 }).unwrap().value,
            Some(CellValue::Error("#DIV/0!".into()))
        );
        assert_eq!(output.pending_shared_strings.len(), 1);
        assert_eq!(output.pending_shared_strings[0].index, 0);
        assert_eq!(
            output.pending_shared_strings[0].cell_ref,
            CellRef { row: 1, col: 2 }
        );
    }

    #[test]
    fn shared_string_cell_records_pending_entry_and_none_value() {
        let xml = br#"<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>5</v></c></row></sheetData></worksheet>"#;
        let (sheet, output) = parse(xml);
        let cell_ref = CellRef { row: 1, col: 1 };
        assert_eq!(sheet.get(cell_ref).unwrap().value, None);
        assert_eq!(output.pending_shared_strings.len(), 1);
        assert_eq!(output.pending_shared_strings[0].cell_ref, cell_ref);
        assert_eq!(output.pending_shared_strings[0].index, 5);
    }

    #[test]
    fn t_d_cell_resolves_all_three_iso8601_shapes() {
        let xml = br#"<worksheet><sheetData><row r="1">
<c r="A1" t="d"><v>2021-01-01</v></c>
<c r="B1" t="d"><v>2021-01-01T10:10:10</v></c>
<c r="C1" t="d"><v>10:10:10</v></c>
</row></sheetData></worksheet>"#;
        let (sheet, _output) = parse(xml);

        assert_eq!(
            sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
            Some(CellValue::DateTime(DateTimeValue {
                year: 2021,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            }))
        );
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 2 }).unwrap().value,
            Some(CellValue::DateTime(DateTimeValue {
                year: 2021,
                month: 1,
                day: 1,
                hour: 10,
                minute: 10,
                second: 10,
            }))
        );
        // Time-only: no date component in the source, so it lands on
        // Excel's "time of day" convention (serial day 0 = 1899-12-30).
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 3 }).unwrap().value,
            Some(CellValue::DateTime(DateTimeValue {
                year: 1899,
                month: 12,
                day: 30,
                hour: 10,
                minute: 10,
                second: 10,
            }))
        );
    }

    #[test]
    fn t_d_cell_with_utc_suffix_drops_the_z() {
        // PR #80 review point 1: a trailing `Z` is a spec-valid UTC
        // designator, not something ECMA-376 writers are barred from
        // emitting. Dropped rather than rejected, since DateTimeValue has
        // no timezone field to convert it into.
        let xml = br#"<worksheet><sheetData><row r="1"><c r="A1" t="d"><v>2021-01-01T10:10:10Z</v></c></row></sheetData></worksheet>"#;
        let (sheet, _output) = parse(xml);
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
            Some(CellValue::DateTime(DateTimeValue {
                year: 2021,
                month: 1,
                day: 1,
                hour: 10,
                minute: 10,
                second: 10,
            }))
        );
    }

    #[test]
    fn t_d_cell_with_offset_suffix_drops_the_offset() {
        let xml = br#"<worksheet><sheetData><row r="1"><c r="A1" t="d"><v>2021-01-01T10:10:10+09:00</v></c></row></sheetData></worksheet>"#;
        let (sheet, _output) = parse(xml);
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
            Some(CellValue::DateTime(DateTimeValue {
                year: 2021,
                month: 1,
                day: 1,
                hour: 10,
                minute: 10,
                second: 10,
            }))
        );
    }

    #[test]
    fn t_d_cell_with_fractional_seconds_truncates_to_whole_seconds() {
        let xml = br#"<worksheet><sheetData><row r="1"><c r="A1" t="d"><v>2021-01-01T10:10:10.500</v></c></row></sheetData></worksheet>"#;
        let (sheet, _output) = parse(xml);
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
            Some(CellValue::DateTime(DateTimeValue {
                year: 2021,
                month: 1,
                day: 1,
                hour: 10,
                minute: 10,
                second: 10,
            }))
        );
    }

    #[test]
    fn t_d_cell_with_omitted_seconds_defaults_to_zero() {
        let xml = br#"<worksheet><sheetData><row r="1"><c r="A1" t="d"><v>10:10</v></c></row></sheetData></worksheet>"#;
        let (sheet, _output) = parse(xml);
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
            Some(CellValue::DateTime(DateTimeValue {
                year: 1899,
                month: 12,
                day: 30,
                hour: 10,
                minute: 10,
                second: 0,
            }))
        );
    }

    #[test]
    fn t_d_cell_time_only_default_date_follows_date1904() {
        // PR #80 review point 2: a time-only ISO date cell's placeholder
        // date must agree with what a numeric time-only cell in the same
        // book would resolve to (resolve/style.rs's EPOCH_OFFSET_1904).
        let xml = br#"<worksheet><sheetData><row r="1"><c r="A1" t="d"><v>10:10:10</v></c></row></sheetData></worksheet>"#;
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        parse_worksheet(
            &xml[..],
            "xl/worksheets/sheet1.xml",
            &mut sheet,
            true,
            usize::MAX,
        )
        .unwrap();
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
            Some(CellValue::DateTime(DateTimeValue {
                year: 1904,
                month: 1,
                day: 1,
                hour: 10,
                minute: 10,
                second: 10,
            }))
        );
    }

    #[test]
    fn t_d_cell_with_wrong_date_segment_count_is_an_error() {
        let xml = br#"<worksheet><sheetData><row r="1"><c r="A1" t="d"><v>2021-01</v></c></row></sheetData></worksheet>"#;
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = parse_worksheet(
            &xml[..],
            "xl/worksheets/sheet1.xml",
            &mut sheet,
            false,
            usize::MAX,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidPackage(_)));
    }

    #[test]
    fn t_d_cell_with_wrong_time_segment_count_is_an_error() {
        let xml = br#"<worksheet><sheetData><row r="1"><c r="A1" t="d"><v>10:10:10:10</v></c></row></sheetData></worksheet>"#;
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = parse_worksheet(
            &xml[..],
            "xl/worksheets/sheet1.xml",
            &mut sheet,
            false,
            usize::MAX,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidPackage(_)));
    }

    #[test]
    fn t_d_cell_with_out_of_range_component_is_an_error() {
        let xml = br#"<worksheet><sheetData><row r="1"><c r="A1" t="d"><v>2021-13-01</v></c></row></sheetData></worksheet>"#;
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = parse_worksheet(
            &xml[..],
            "xl/worksheets/sheet1.xml",
            &mut sheet,
            false,
            usize::MAX,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidPackage(_)));
    }

    #[test]
    fn t_d_cell_with_malformed_time_is_an_error() {
        let xml = br#"<worksheet><sheetData><row r="1"><c r="A1" t="d"><v>25:00:00</v></c></row></sheetData></worksheet>"#;
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = parse_worksheet(
            &xml[..],
            "xl/worksheets/sheet1.xml",
            &mut sheet,
            false,
            usize::MAX,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidPackage(_)));
    }

    #[test]
    fn styled_cell_records_pending_style() {
        let xml = br#"<worksheet><sheetData><row r="1"><c r="A1" s="3"><v>1</v></c></row></sheetData></worksheet>"#;
        let (_sheet, output) = parse(xml);
        assert_eq!(output.pending_styles.len(), 1);
        assert_eq!(
            output.pending_styles[0].cell_ref,
            CellRef { row: 1, col: 1 }
        );
        assert_eq!(output.pending_styles[0].style_id, 3);
    }

    #[test]
    fn str_cell_resolves_directly_without_pending_entry() {
        let xml = br#"<worksheet><sheetData><row r="1"><c r="A1" t="str"><f>A2+A3</f><v>computed</v></c></row></sheetData></worksheet>"#;
        let (sheet, output) = parse(xml);
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
            Some(CellValue::Text(Arc::from("computed")))
        );
        assert!(output.pending_shared_strings.is_empty());
    }

    #[test]
    fn inline_str_simple_and_rich_text() {
        let xml = br#"<worksheet><sheetData><row r="1">
<c r="A1" t="inlineStr"><is><t>plain</t></is></c>
<c r="B1" t="inlineStr"><is><r><t xml:space="preserve">rich </t></r><r><t>text</t></r></is></c>
</row></sheetData></worksheet>"#;
        let (sheet, _output) = parse(xml);
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
            Some(CellValue::Text(Arc::from("plain")))
        );
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 2 }).unwrap().value,
            Some(CellValue::Text(Arc::from("rich text")))
        );
    }

    #[test]
    fn fully_blank_cell_is_not_inserted() {
        let xml = br#"<worksheet><sheetData><row r="1"><c r="A1"/></row></sheetData></worksheet>"#;
        let (sheet, _output) = parse(xml);
        assert!(sheet.get(CellRef { row: 1, col: 1 }).is_none());
    }

    #[test]
    fn empty_row_inserts_nothing() {
        let xml = br#"<worksheet><sheetData><row r="1"></row></sheetData></worksheet>"#;
        let (sheet, output) = parse(xml);
        assert_eq!(sheet.max_row, 0);
        assert!(output.pending_shared_strings.is_empty());
    }

    #[test]
    fn no_state_leaks_across_rows() {
        // Row 1 has a t="s" cell; row 2's plain numeric cell at the same
        // column must not be mistaken for a shared-string reference.
        let xml = br#"<worksheet><sheetData>
<row r="1"><c r="A1" t="s"><v>0</v></c></row>
<row r="2"><c r="A2"><v>7</v></c></row>
</sheetData></worksheet>"#;
        let (sheet, output) = parse(xml);
        assert_eq!(output.pending_shared_strings.len(), 1);
        assert_eq!(
            sheet.get(CellRef { row: 2, col: 1 }).unwrap().value,
            Some(CellValue::Number(7.0))
        );
    }

    #[test]
    fn merge_cells_collected_with_correct_bounds() {
        let xml = br#"<worksheet><sheetData></sheetData>
<mergeCells count="2"><mergeCell ref="A1:C3"/><mergeCell ref="D4:D4"/></mergeCells>
</worksheet>"#;
        let (_sheet, output) = parse(xml);
        assert_eq!(output.merge_regions.len(), 2);
        assert_eq!(
            output.merge_regions[0],
            MergedRegion {
                start: CellRef { row: 1, col: 1 },
                end: CellRef { row: 3, col: 3 },
            }
        );
        assert_eq!(
            output.merge_regions[1],
            MergedRegion {
                start: CellRef { row: 4, col: 4 },
                end: CellRef { row: 4, col: 4 },
            }
        );
    }

    #[test]
    fn merge_cells_with_reversed_start_end_are_passed_through_unvalidated() {
        let xml = br#"<worksheet><sheetData></sheetData>
<mergeCells><mergeCell ref="C3:A1"/></mergeCells>
</worksheet>"#;
        let (_sheet, output) = parse(xml);
        assert_eq!(output.merge_regions.len(), 1);
        assert_eq!(output.merge_regions[0].start, CellRef { row: 3, col: 3 });
        assert_eq!(output.merge_regions[0].end, CellRef { row: 1, col: 1 });
    }

    #[test]
    fn cols_are_collected_with_min_max_width() {
        let xml = br#"<worksheet><cols>
<col min="1" max="3" width="12.5" customWidth="1"/>
<col min="5" max="5" width="8.43"/>
</cols><sheetData></sheetData></worksheet>"#;
        let (_sheet, output) = parse(xml);
        assert_eq!(
            output.col_width_ranges,
            vec![
                ColWidthRange {
                    min: 1,
                    max: 3,
                    width: 12.5
                },
                ColWidthRange {
                    min: 5,
                    max: 5,
                    width: 8.43
                },
            ]
        );
    }

    #[test]
    fn col_without_width_attribute_is_ignored() {
        // A <col> that only sets e.g. `hidden`/`bestFit` without an
        // explicit width carries nothing this library tracks.
        let xml = br#"<worksheet><cols><col min="1" max="1" hidden="1"/></cols><sheetData></sheetData></worksheet>"#;
        let (_sheet, output) = parse(xml);
        assert_eq!(output.col_width_ranges, vec![]);
    }

    #[test]
    fn a_single_full_width_col_does_not_expand_into_16384_ranges() {
        // The realistic-worst-case shape: one <col> spanning the entire
        // column range. Must be collected as exactly one ColWidthRange
        // (Issue #39's performance requirement), proven here at the
        // parse-output level.
        let xml = br#"<worksheet><cols><col min="1" max="16384" width="8.43"/></cols><sheetData></sheetData></worksheet>"#;
        let (_sheet, output) = parse(xml);
        assert_eq!(output.col_width_ranges.len(), 1);
        assert_eq!(output.col_width_ranges[0].min, 1);
        assert_eq!(output.col_width_ranges[0].max, 16_384);
    }

    #[test]
    fn sheet_format_pr_default_col_width_is_collected() {
        let xml = br#"<worksheet><sheetFormatPr defaultColWidth="9.1" defaultRowHeight="15"/><sheetData></sheetData></worksheet>"#;
        let (_sheet, output) = parse(xml);
        assert_eq!(output.default_col_width, Some(9.1));
    }

    #[test]
    fn missing_sheet_format_pr_leaves_default_col_width_none() {
        let xml = br#"<worksheet><sheetData></sheetData></worksheet>"#;
        let (_sheet, output) = parse(xml);
        assert_eq!(output.default_col_width, None);
    }

    #[test]
    fn malformed_col_width_is_invalid_package() {
        let xml = br#"<worksheet><cols><col min="1" max="1" width="not-a-number"/></cols><sheetData></sheetData></worksheet>"#;
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = parse_worksheet(
            &xml[..],
            "xl/worksheets/sheet1.xml",
            &mut sheet,
            false,
            usize::MAX,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidPackage(_)));
    }

    #[test]
    fn cell_missing_r_infers_sequential_columns() {
        // Issue #79: `r` is optional per ECMA-376 §18.3.1.4; when omitted,
        // the column is inferred as one past the previous cell in the row.
        // Mirrors the exact shape of tests/fixtures/other/minimal_package.xlsx
        // (calamine test corpus, not committed — see that directory's
        // .gitignore entry): every <row> carries `r`, no <c> does.
        let xml = br#"<worksheet><sheetData><row r="1">
<c t="n"><v>1</v></c>
<c t="n"><v>2</v></c>
<c t="n"><v>3</v></c>
</row></sheetData></worksheet>"#;
        let (sheet, _output) = parse(xml);
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
            Some(CellValue::Number(1.0))
        );
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 2 }).unwrap().value,
            Some(CellValue::Number(2.0))
        );
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 3 }).unwrap().value,
            Some(CellValue::Number(3.0))
        );
    }

    #[test]
    fn cell_missing_r_after_explicit_r_resumes_from_that_column() {
        // A later cell that does carry `r` is authoritative; a subsequent
        // omitted-`r` cell resumes counting from there, not from the start
        // of the row.
        let xml = br#"<worksheet><sheetData><row r="1">
<c t="n"><v>1</v></c>
<c r="E1" t="n"><v>5</v></c>
<c t="n"><v>6</v></c>
</row></sheetData></worksheet>"#;
        let (sheet, _output) = parse(xml);
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
            Some(CellValue::Number(1.0))
        );
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 5 }).unwrap().value,
            Some(CellValue::Number(5.0))
        );
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 6 }).unwrap().value,
            Some(CellValue::Number(6.0))
        );
    }

    #[test]
    fn row_missing_r_infers_sequential_row() {
        let xml = br#"<worksheet><sheetData>
<row r="1"><c t="n"><v>1</v></c></row>
<row><c t="n"><v>2</v></c></row>
</sheetData></worksheet>"#;
        let (sheet, _output) = parse(xml);
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
            Some(CellValue::Number(1.0))
        );
        assert_eq!(
            sheet.get(CellRef { row: 2, col: 1 }).unwrap().value,
            Some(CellValue::Number(2.0))
        );
    }

    #[test]
    fn cell_missing_r_with_no_enclosing_row_is_invalid_cell_ref() {
        // Malformed nesting (schema-invalid, but this SAX-style parser does
        // not validate structure): a <c> with no <row> at all has no row
        // context to infer a position from.
        let xml = br#"<worksheet><sheetData><c t="n"><v>1</v></c></sheetData></worksheet>"#;
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = parse_worksheet(
            &xml[..],
            "xl/worksheets/sheet1.xml",
            &mut sheet,
            false,
            usize::MAX,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidCellRef(_)));
    }

    #[test]
    fn cell_missing_r_beyond_max_col_is_invalid_cell_ref() {
        // Security review docs/security/code-review.md Finding 2: an
        // inferred column must be bounded the same way `CellRef::from_a1`
        // already bounds an explicit one, or an attacker could inflate
        // `Sheet::max_col` (and json.rs's `maxCol`) far past
        // `CellRef::MAX_COL` using nothing but MAX_COL + 1 empty <c/>
        // elements omitting `r` — no `r`-based coordinate ever appears in
        // the input for `CellRef::from_a1`'s own check to catch.
        let mut cells = String::new();
        for _ in 0..=CellRef::MAX_COL {
            cells.push_str("<c/>");
        }
        let xml =
            format!(r#"<worksheet><sheetData><row r="1">{cells}</row></sheetData></worksheet>"#);
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = parse_worksheet(
            xml.as_bytes(),
            "xl/worksheets/sheet1.xml",
            &mut sheet,
            false,
            usize::MAX,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidCellRef(_)));
    }

    #[test]
    fn row_r_attribute_non_numeric_is_invalid_cell_ref() {
        // PR #81 review: a non-numeric <row r="..."> used to fall through
        // parse_u32_attr and surface as Error::InvalidPackage, inconsistent
        // with how a malformed <c r="..."> is reported via
        // CellRef::from_a1's Error::InvalidCellRef.
        let xml = br#"<worksheet><sheetData><row r="abc"><c t="n"><v>1</v></c></row></sheetData></worksheet>"#;
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = parse_worksheet(
            &xml[..],
            "xl/worksheets/sheet1.xml",
            &mut sheet,
            false,
            usize::MAX,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidCellRef(_)));
    }

    #[test]
    fn row_r_attribute_of_zero_is_invalid_cell_ref() {
        // Rows are 1-based, same as CellRef::from_a1 already enforces for
        // an explicit <c r="...">; an explicit <row r="0"> must be rejected
        // the same way rather than silently becoming a valid row.
        let xml = br#"<worksheet><sheetData><row r="0"><c t="n"><v>1</v></c></row></sheetData></worksheet>"#;
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = parse_worksheet(
            &xml[..],
            "xl/worksheets/sheet1.xml",
            &mut sheet,
            false,
            usize::MAX,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidCellRef(_)));
    }

    #[test]
    fn row_r_attribute_beyond_max_row_is_invalid_cell_ref() {
        let xml = format!(
            r#"<worksheet><sheetData><row r="{}"><c t="n"><v>1</v></c></row></sheetData></worksheet>"#,
            CellRef::MAX_ROW + 1
        );
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = parse_worksheet(
            xml.as_bytes(),
            "xl/worksheets/sheet1.xml",
            &mut sheet,
            false,
            usize::MAX,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidCellRef(_)));
    }

    #[test]
    fn malformed_cell_ref_is_invalid_cell_ref() {
        let xml = br#"<worksheet><sheetData><row r="1"><c r="1A"><v>1</v></c></row></sheetData></worksheet>"#;
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = parse_worksheet(
            &xml[..],
            "xl/worksheets/sheet1.xml",
            &mut sheet,
            false,
            usize::MAX,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidCellRef(_)));
    }

    #[test]
    fn malformed_merge_ref_is_invalid_cell_ref() {
        let xml = br#"<worksheet><sheetData></sheetData>
<mergeCells><mergeCell ref="notarange"/></mergeCells>
</worksheet>"#;
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = parse_worksheet(
            &xml[..],
            "xl/worksheets/sheet1.xml",
            &mut sheet,
            false,
            usize::MAX,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidCellRef(_)));
    }

    #[test]
    fn formula_cell_uses_cached_value_and_ignores_formula_text() {
        let xml = br#"<worksheet><sheetData><row r="1"><c r="A1"><f>SUM(A2:A10)</f><v>55</v></c></row></sheetData></worksheet>"#;
        let (sheet, _output) = parse(xml);
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
            Some(CellValue::Number(55.0))
        );
    }
}
