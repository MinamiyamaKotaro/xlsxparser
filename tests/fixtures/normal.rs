//! Category 1 (正常系, `tests/fixtures/normal/` in Issue #28): the
//! library's core read → JSON-mapping path, with no error paths involved.

use super::builder::*;

/// A single sheet covering every `CellValue` variant in one row: text
/// (`t="str"`), integer, decimal, a date serial value styled with a
/// built-in date numFmt, boolean TRUE/FALSE, and an error code
/// (`t="e"`). Verifies each OOXML cell type maps to the right JSON `type`
/// tag.
pub fn basic_types() -> Vec<u8> {
    let rows = r#"<row r="1">
  <c r="A1" t="str"><v>日本語Text</v></c>
  <c r="B1"><v>42</v></c>
  <c r="C1"><v>19.99</v></c>
  <c r="D1" s="1"><v>45000</v></c>
  <c r="E1" t="b"><v>1</v></c>
  <c r="F1" t="b"><v>0</v></c>
  <c r="G1" t="e"><v>#N/A</v></c>
</row>"#;

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
        ("xl/styles.xml", DATE_STYLES_XML),
        (
            "xl/worksheets/sheet1.xml",
            worksheet_xml(rows, "").as_bytes(),
        ),
    ])
}

/// Two cells, each styled via a distinct `<cellXfs>` entry that resolves
/// (via `fontId`) to a different `<fonts>` entry: A1 uses the default
/// 11pt/not-bold font (style id 0), B1 uses a 14pt/bold font (style id 1,
/// a plausible "heading" style). Verifies `<fonts>`/`fontId` resolution and
/// the per-cell `style.font` JSON output end to end (Issue #38).
pub fn font_styles() -> Vec<u8> {
    let rows = r#"<row r="1">
  <c r="A1" s="0"><v>1</v></c>
  <c r="B1" s="1"><v>2</v></c>
</row>"#;

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
        ("xl/styles.xml", FONT_STYLES_XML),
        (
            "xl/worksheets/sheet1.xml",
            worksheet_xml(rows, "").as_bytes(),
        ),
    ])
}

/// A sheet with `<cols>` declaring two width ranges (one covering columns
/// A-C, one covering just E) plus `<sheetFormatPr defaultColWidth="..">`
/// for every other column, alongside a couple of populated cells. Verifies
/// `<cols>`/`<sheetFormatPr>` parsing and the sheet-level `columns` JSON
/// output end to end (Issue #39).
pub fn column_widths() -> Vec<u8> {
    let worksheet = r#"<worksheet>
<sheetFormatPr defaultColWidth="9.1" defaultRowHeight="15"/>
<cols>
<col min="1" max="3" width="12.5" customWidth="1"/>
<col min="5" max="5" width="30"/>
</cols>
<sheetData>
<row r="1">
  <c r="A1"><v>1</v></c>
  <c r="E1"><v>2</v></c>
</row>
</sheetData>
</worksheet>"#;

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
        ("xl/worksheets/sheet1.xml", worksheet.as_bytes()),
    ])
}

/// The same string ("Apple") is referenced by two different cells via the
/// Shared String Table, alongside one cell referencing a different string
/// ("Banana"). Verifies `<v>` shared-string indices resolve to the correct
/// text, including reuse of a single SST entry from more than one cell.
pub fn shared_strings() -> Vec<u8> {
    let rows = r#"<row r="1">
  <c r="A1" t="s"><v>0</v></c>
  <c r="B1" t="s"><v>1</v></c>
  <c r="C1" t="s"><v>0</v></c>
</row>"#;

    build_zip(&[
        (
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[
                ("rId1", "worksheet", "worksheets/sheet1.xml"),
                ("rId2", "styles", "styles.xml"),
                ("rId3", "sharedStrings", "sharedStrings.xml"),
            ])
            .as_bytes(),
        ),
        (
            "xl/workbook.xml",
            workbook_xml(&[("Sheet1", "rId1", None)]).as_bytes(),
        ),
        ("xl/styles.xml", DEFAULT_STYLES_XML),
        (
            "xl/sharedStrings.xml",
            shared_strings_xml(&["Apple", "Banana"]).as_bytes(),
        ),
        (
            "xl/worksheets/sheet1.xml",
            worksheet_xml(rows, "").as_bytes(),
        ),
    ])
}

/// Cells carry their string directly (`t="inlineStr"`, `<is><t>...</t></is>`)
/// instead of going through the Shared String Table — the shape third-party
/// (non-Excel) OOXML writers tend to emit. No `sharedStrings.xml` part
/// exists in this package at all (and no relationship pointing to one),
/// exercising the pipeline's "SST part is genuinely absent" fallback path.
/// Verifies inline strings are extracted the same way shared strings are.
pub fn inline_strings() -> Vec<u8> {
    let rows = r#"<row r="1">
  <c r="A1" t="inlineStr"><is><t>Inline One</t></is></c>
  <c r="B1" t="inlineStr"><is><t>Inline Two</t></is></c>
</row>"#;

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
