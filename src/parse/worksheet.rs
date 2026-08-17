//! Phase 3: SAX-style streaming parse of `xl/worksheets/sheetX.xml`.
//! Streams `<sheetData>` row by row, inserting cells into `Sheet` directly,
//! while collecting the pieces that need Phase 4's deferred resolution
//! (shared strings, styles, merged ranges).

use crate::error::Error;
use crate::model::{Cell, CellRef, CellValue, MergedRegion, Sheet, StyleId};
use crate::parse::{
    concat_rich_text, create_secure_reader, optional_attr, push_general_ref, read_event,
    required_attr,
};
use quick_xml::events::Event;
use quick_xml::Reader;
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

/// `parse_worksheet`'s output. `sheet` itself is mutated directly through
/// the `&mut` argument, so this only returns the three remaining pieces of
/// unresolved data Phase 4 needs.
#[derive(Debug)]
pub(crate) struct WorksheetParseOutput {
    pub pending_shared_strings: Vec<PendingSharedString>,
    pub pending_styles: Vec<PendingStyle>,
    pub merge_regions: Vec<MergedRegion>,
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
pub(crate) fn parse_worksheet(
    reader: impl BufRead,
    path: &str,
    sheet: &mut Sheet,
) -> Result<WorksheetParseOutput, Error> {
    let mut xml_reader = create_secure_reader(reader);
    let mut buf = Vec::new();
    let mut pending_shared_strings = Vec::new();
    let mut pending_styles = Vec::new();
    let mut merge_regions = Vec::new();

    // State for the `<c>` currently being read (between its start and end
    // tag). `cur_ref` doubles as "are we inside a <c>?".
    let mut cur_ref: Option<CellRef> = None;
    let mut cur_type: Option<String> = None;
    let mut cur_style: Option<u32> = None;
    let mut cur_value_text: Option<String> = None;
    let mut cur_inline: Option<String> = None;

    loop {
        let event = read_event(&mut xml_reader, &mut buf, path)?;
        match &event {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"c" => {
                let is_empty = matches!(&event, Event::Empty(_));
                let r = required_attr(e, path, "r")?;
                let cell_ref = CellRef::from_a1(&r)?;
                let cell_type = optional_attr(e, path, "t")?;
                let style_id = optional_attr(e, path, "s")?.and_then(|s| s.parse::<u32>().ok());

                if is_empty {
                    flush_cell(
                        sheet,
                        &mut pending_shared_strings,
                        &mut pending_styles,
                        path,
                        cell_ref,
                        cell_type,
                        style_id,
                        None,
                        None,
                    )?;
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
                    flush_cell(
                        sheet,
                        &mut pending_shared_strings,
                        &mut pending_styles,
                        path,
                        cell_ref,
                        cur_type.take(),
                        cur_style.take(),
                        cur_value_text.take(),
                        cur_inline.take(),
                    )?;
                }
            }
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"mergeCell" => {
                let cell_range = required_attr(e, path, "ref")?;
                merge_regions.push(parse_merge_ref(&cell_range)?);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(WorksheetParseOutput {
        pending_shared_strings,
        pending_styles,
        merge_regions,
    })
}

/// Finalizes one `<c>`: decides whether it carries enough information to be
/// worth inserting at all (a sparse matrix never instantiates a fully blank
/// cell — no value, no style, not even a pending shared-string reference),
/// and if so, inserts it and records any deferred-resolution entries.
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
) -> Result<(), Error> {
    let is_shared_string = cell_type.as_deref() == Some("s");
    let has_value = value_text.is_some() || inline_string.is_some();

    if style_id.is_none() && !has_value && !is_shared_string {
        return Ok(());
    }

    let cell = build_cell(
        cell_type.as_deref(),
        value_text.as_deref(),
        inline_string,
        path,
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

/// Reads the text content of a leaf element (`<v>...</v>`, `<f>...</f>`) —
/// no nested elements are expected — resolving any `Event::GeneralRef`
/// entities along the way. Called with the reader positioned just after the
/// element's opening tag; consumes events up to and including its closing
/// tag.
fn read_leaf_text(reader: &mut Reader<impl BufRead>, path: &str) -> Result<String, Error> {
    let mut text = String::new();
    let mut buf = Vec::new();
    loop {
        match read_event(reader, &mut buf, path)? {
            Event::Text(e) => {
                let decoded = e.decode().map_err(|err| Error::XmlParse {
                    path: path.to_string(),
                    source: Box::new(err),
                })?;
                text.push_str(&decoded);
            }
            Event::GeneralRef(e) => push_general_ref(&mut text, &e, path)?,
            Event::End(_) => break,
            Event::Eof => {
                return Err(Error::MissingRequiredElement {
                    path: path.to_string(),
                    name: "closing tag",
                })
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SheetVisibility;

    fn parse(xml: &[u8]) -> (Sheet, WorksheetParseOutput) {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let output = parse_worksheet(xml, "xl/worksheets/sheet1.xml", &mut sheet).unwrap();
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
<c r="B1" t="inlineStr"><is><r><t>rich </t></r><r><t>text</t></r></is></c>
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
    fn cell_missing_r_is_an_error() {
        let xml = br#"<worksheet><sheetData><row r="1"><c t="n"><v>1</v></c></row></sheetData></worksheet>"#;
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = parse_worksheet(&xml[..], "xl/worksheets/sheet1.xml", &mut sheet).unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement { name: "r", .. }
        ));
    }

    #[test]
    fn malformed_cell_ref_is_invalid_cell_ref() {
        let xml = br#"<worksheet><sheetData><row r="1"><c r="1A"><v>1</v></c></row></sheetData></worksheet>"#;
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = parse_worksheet(&xml[..], "xl/worksheets/sheet1.xml", &mut sheet).unwrap_err();
        assert!(matches!(err, Error::InvalidCellRef(_)));
    }

    #[test]
    fn malformed_merge_ref_is_invalid_cell_ref() {
        let xml = br#"<worksheet><sheetData></sheetData>
<mergeCells><mergeCell ref="notarange"/></mergeCells>
</worksheet>"#;
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = parse_worksheet(&xml[..], "xl/worksheets/sheet1.xml", &mut sheet).unwrap_err();
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
