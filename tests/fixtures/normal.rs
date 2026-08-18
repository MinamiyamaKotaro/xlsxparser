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

/// Two cells: A1's `<cellXfs>` entry (style id 0) carries an
/// `<alignment wrapText="1"/>` child, B1's (style id 1) is a self-closing
/// `<xf/>` with no alignment at all. Verifies `<alignment wrapText>`
/// resolution — including that the `<xf>` Start/End restructuring needed
/// to support a child element didn't regress the plain self-closing
/// form — and the per-cell `style.wrapText` JSON output end to end (Issue
/// #37).
pub fn wrap_text_styles() -> Vec<u8> {
    let rows = r#"<row r="1">
  <c r="A1" s="0"><v>1</v></c>
  <c r="B1" s="1"><v>2</v></c>
</row>"#;

    let styles = br#"<styleSheet><cellXfs>
<xf numFmtId="0"><alignment wrapText="1"/></xf>
<xf numFmtId="0"/>
</cellXfs></styleSheet>"#;

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
        ("xl/styles.xml", styles),
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

/// Two cells: A1's `<cellXfs>` entry (style id 0) carries
/// `<alignment horizontal="center"/>`, B1's (style id 1) is a self-closing
/// `<xf/>` with no alignment at all (so it resolves to the "general"
/// default). Verifies `<alignment horizontal>` resolution and the per-cell
/// `style.alignment` JSON output end to end (Issue #42).
pub fn alignment_styles() -> Vec<u8> {
    let rows = r#"<row r="1">
  <c r="A1" s="0"><v>1</v></c>
  <c r="B1" s="1"><v>2</v></c>
</row>"#;

    let styles = br#"<styleSheet><cellXfs>
<xf numFmtId="0"><alignment horizontal="center"/></xf>
<xf numFmtId="0"/>
</cellXfs></styleSheet>"#;

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
        ("xl/styles.xml", styles),
        (
            "xl/worksheets/sheet1.xml",
            worksheet_xml(rows, "").as_bytes(),
        ),
    ])
}

/// Two cells: A1 styled with a built-in percentage format (`numFmtId="9"`,
/// `"0%"`), B1 styled with a custom currency-ish format declared in
/// `<numFmts>`. Verifies built-in and custom `numFmtId` resolution and the
/// per-cell `style.numberFormat` JSON output end to end (Issue #41).
pub fn number_format_styles() -> Vec<u8> {
    let rows = r#"<row r="1">
  <c r="A1" s="0"><v>0.5</v></c>
  <c r="B1" s="1"><v>1234.5</v></c>
</row>"#;

    let styles = r##"<styleSheet>
<numFmts><numFmt numFmtId="164" formatCode="#,##0.00 &quot;円&quot;"/></numFmts>
<cellXfs>
<xf numFmtId="9"/>
<xf numFmtId="164"/>
</cellXfs>
</styleSheet>"##;

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
        ("xl/styles.xml", styles.as_bytes()),
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

/// A single date cell (serial 1, styled with a built-in date numFmt) in a
/// workbook whose `<workbookPr date1904="1"/>` selects the 1904 date
/// system. Verifies `date1904` is threaded end to end from
/// `parse/workbook.rs` through `pipeline.rs` into `resolve/style.rs`'s
/// epoch selection (Issue #40) — serial 1 resolves to 1904-01-02 here,
/// versus 1900-01-01 under the (unset, default) 1900 system exercised by
/// `basic_types()`.
pub fn date1904_styles() -> Vec<u8> {
    let workbook_xml = br#"<?xml version="1.0"?>
<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <workbookPr date1904="1"/>
  <sheets>
    <sheet name="Sheet1" sheetId="1" r:id="rId1"/>
  </sheets>
</workbook>"#;
    let rows = r#"<row r="1">
  <c r="A1" s="1"><v>1</v></c>
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
        ("xl/workbook.xml", workbook_xml),
        ("xl/styles.xml", DATE_STYLES_XML),
        (
            "xl/worksheets/sheet1.xml",
            worksheet_xml(rows, "").as_bytes(),
        ),
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
