//! Category 3 (正常系(複合型), `tests/fixtures/complex/` in Issue #28):
//! structures specific to Japanese business spreadsheets ("houganshi"
//! grid-paper Excel, sheets with mixed visibility, extremely sparse
//! coordinate ranges) that only appear once multiple pipeline phases
//! interact.

use super::builder::*;

/// A merged region spans `A1:C3` (3 rows x 3 cols) with its value only set
/// on the anchor cell `A1`. Verifies Phase 4's resolution makes every
/// coordinate inside the region (e.g. `C3`) resolve to the same anchor
/// cell, and that the JSON output reports `rowSpan`/`colSpan` on the
/// anchor while the virtual coordinates never appear as separate JSON
/// cells (see `json.rs`'s `merged_cell_reports_span_and_excludes_virtual_coordinates`
/// unit test for the equivalent in-memory-model assertion this mirrors
/// end-to-end).
pub fn houganshi_merged() -> Vec<u8> {
    let rows = r#"<row r="1"><c r="A1" t="str"><v>houganshi</v></c></row>"#;
    let merge_cells = r#"<mergeCells count="1"><mergeCell ref="A1:C3"/></mergeCells>"#;

    build_zip(&[
        (
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[
                ("rId1", "worksheet", "worksheets/sheet1.xml"),
                ("rId2", "styles", "styles.xml"),
            ])
            .as_bytes(),
        ),
        (
            "xl/workbook.xml",
            workbook_xml(&[("Sheet1", "rId1", None)]).as_bytes(),
        ),
        ("xl/styles.xml", DEFAULT_STYLES_XML),
        (
            "xl/worksheets/sheet1.xml",
            worksheet_xml(rows, merge_cells).as_bytes(),
        ),
    ])
}

/// Four sheets with every `SheetVisibility` state Excel allows, plus one
/// sheet with no data at all: `Visible`, `Hidden` (`state="hidden"`),
/// `VeryHidden` (`state="veryHidden"`), and `Empty` (present in
/// `workbook.xml` but its `<sheetData>` has no rows). Verifies all four are
/// enumerated in the JSON output — none dropped for being hidden or empty.
pub fn multi_sheet_states() -> Vec<u8> {
    let visible_rows = r#"<row r="1"><c r="A1"><v>1</v></c></row>"#;
    let blank = "";

    build_zip(&[
        (
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[
                ("rId1", "worksheet", "worksheets/sheet1.xml"),
                ("rId2", "worksheet", "worksheets/sheet2.xml"),
                ("rId3", "worksheet", "worksheets/sheet3.xml"),
                ("rId4", "worksheet", "worksheets/sheet4.xml"),
                ("rId5", "styles", "styles.xml"),
            ])
            .as_bytes(),
        ),
        (
            "xl/workbook.xml",
            workbook_xml(&[
                ("Visible", "rId1", None),
                ("Hidden", "rId2", Some("hidden")),
                ("VeryHidden", "rId3", Some("veryHidden")),
                ("Empty", "rId4", None),
            ])
            .as_bytes(),
        ),
        ("xl/styles.xml", DEFAULT_STYLES_XML),
        (
            "xl/worksheets/sheet1.xml",
            worksheet_xml(visible_rows, "").as_bytes(),
        ),
        (
            "xl/worksheets/sheet2.xml",
            worksheet_xml(visible_rows, "").as_bytes(),
        ),
        (
            "xl/worksheets/sheet3.xml",
            worksheet_xml(visible_rows, "").as_bytes(),
        ),
        (
            "xl/worksheets/sheet4.xml",
            worksheet_xml(blank, "").as_bytes(),
        ),
    ])
}

/// `A1` holds a value, and the next (and only other) populated cell is
/// `XFD1048576` — Excel's absolute bottom-right corner (column 16384, row
/// 1,048,576). Verifies the sparse `HashMap<CellRef, Cell>` storage means
/// only the 2 populated coordinates are ever registered — the ~1.7 trillion
/// cells in between are never iterated, allocated, or represented.
pub fn extreme_sparse() -> Vec<u8> {
    let rows = r#"<row r="1"><c r="A1"><v>1</v></c></row>
<row r="1048576"><c r="XFD1048576"><v>2</v></c></row>"#;

    build_zip(&[
        (
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[
                ("rId1", "worksheet", "worksheets/sheet1.xml"),
                ("rId2", "styles", "styles.xml"),
            ])
            .as_bytes(),
        ),
        (
            "xl/workbook.xml",
            workbook_xml(&[("Sheet1", "rId1", None)]).as_bytes(),
        ),
        ("xl/styles.xml", DEFAULT_STYLES_XML),
        (
            "xl/worksheets/sheet1.xml",
            worksheet_xml(rows, "").as_bytes(),
        ),
    ])
}
