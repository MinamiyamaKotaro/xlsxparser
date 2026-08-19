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

/// A picture anchored `B2:E9` (Issue #65), carrying both an embedded media
/// relationship (`r:embed`) and the image's own `External` hyperlink
/// (`a:hlinkClick`) — the two relationship kinds `pipeline.rs`'s Phase 3.5
/// resolves against `drawing1.xml.rels`. Mirrors
/// `scripts/generate_real_fixtures.py`'s `embedded_image()`, the real
/// openpyxl-authored counterpart exercised by `tests/real_fixtures.rs`.
pub fn embedded_image() -> Vec<u8> {
    let rows = r#"<row r="1"><c r="A1" t="str"><v>logo</v></c></row>"#;
    let worksheet = worksheet_xml(rows, "").replace(
        "</worksheet>",
        r#"<drawing r:id="rIdDrawing"/></worksheet>"#,
    );
    let worksheet_rels = rels_xml(&[("rIdDrawing", "drawing", "../drawings/drawing1.xml")]);
    let drawing_xml = br#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <xdr:twoCellAnchor>
    <xdr:from><xdr:col>1</xdr:col><xdr:colOff>10000</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>20000</xdr:rowOff></xdr:from>
    <xdr:to><xdr:col>4</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>8</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>
    <xdr:pic>
      <xdr:nvPicPr>
        <xdr:cNvPr id="2" name="Picture 1"><a:hlinkClick r:id="rIdHyperlink"/></xdr:cNvPr>
        <xdr:cNvPicPr/>
      </xdr:nvPicPr>
      <xdr:blipFill><a:blip r:embed="rIdEmbed"/></xdr:blipFill>
    </xdr:pic>
    <xdr:clientData/>
  </xdr:twoCellAnchor>
</xdr:wsDr>"#;
    let drawing_rels: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdEmbed" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/>
  <Relationship Id="rIdHyperlink" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/sample-image" TargetMode="External"/>
</Relationships>"#;

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
        (
            "xl/worksheets/_rels/sheet1.xml.rels",
            worksheet_rels.as_bytes(),
        ),
        ("xl/drawings/drawing1.xml", drawing_xml),
        ("xl/drawings/_rels/drawing1.xml.rels", drawing_rels),
        // Content doesn't matter — xlsxparser never reads image bytes.
        ("xl/media/image1.png", b"\x89PNG\r\n\x1a\n" as &[u8]),
    ])
}

/// A picture anchored at `C5` via `oneCellAnchor` (Issue #65), sized well
/// under a default cell's EMU dimensions — confined *within* a single cell,
/// contrasting `embedded_image`'s `twoCellAnchor` spanning `B2:E9`. Carries
/// no hyperlink, so `Image::hyperlink` should resolve to `None`.
pub fn embedded_image_one_cell() -> Vec<u8> {
    let rows = r#"<row r="1"><c r="A1" t="str"><v>icon</v></c></row>"#;
    let worksheet = worksheet_xml(rows, "").replace(
        "</worksheet>",
        r#"<drawing r:id="rIdDrawing"/></worksheet>"#,
    );
    let worksheet_rels = rels_xml(&[("rIdDrawing", "drawing", "../drawings/drawing1.xml")]);
    let drawing_xml = br#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <xdr:oneCellAnchor>
    <xdr:from><xdr:col>2</xdr:col><xdr:colOff>5000</xdr:colOff><xdr:row>4</xdr:row><xdr:rowOff>5000</xdr:rowOff></xdr:from>
    <xdr:ext cx="400000" cy="150000"/>
    <xdr:pic>
      <xdr:nvPicPr>
        <xdr:cNvPr id="2" name="Picture 1"/>
        <xdr:cNvPicPr/>
      </xdr:nvPicPr>
      <xdr:blipFill><a:blip r:embed="rIdEmbed"/></xdr:blipFill>
    </xdr:pic>
    <xdr:clientData/>
  </xdr:oneCellAnchor>
</xdr:wsDr>"#;
    let drawing_rels: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdEmbed" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/>
</Relationships>"#;

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
        (
            "xl/worksheets/_rels/sheet1.xml.rels",
            worksheet_rels.as_bytes(),
        ),
        ("xl/drawings/drawing1.xml", drawing_xml),
        ("xl/drawings/_rels/drawing1.xml.rels", drawing_rels),
        // Content doesn't matter — xlsxparser never reads image bytes.
        ("xl/media/image1.png", b"\x89PNG\r\n\x1a\n" as &[u8]),
    ])
}

/// Two pictures grouped together via `<xdr:grpSp>` (Issue #67), using the
/// same numeric conventions confirmed against real LibreOffice output: the
/// outermost group's own `chOff`/`chExt` equal its `off`/`ext` (scale 1),
/// and both group- and pic-level `off`/`ext` are literal absolute-canvas
/// EMU. Mirrors `src/parse/drawing.rs`'s
/// `single_level_group_resolves_each_pic_relative_to_from` unit test, at
/// full-pipeline scope: relationship resolution (`target`/`hyperlink`) on
/// top of the anchor-math the unit test already covers. The first pic
/// carries no hyperlink; the second carries an `External` one, verifying
/// per-pic hyperlink scoping survives the group transform.
pub fn grouped_images() -> Vec<u8> {
    let rows = r#"<row r="1"><c r="A1" t="str"><v>logos</v></c></row>"#;
    let worksheet = worksheet_xml(rows, "").replace(
        "</worksheet>",
        r#"<drawing r:id="rIdDrawing"/></worksheet>"#,
    );
    let worksheet_rels = rels_xml(&[("rIdDrawing", "drawing", "../drawings/drawing1.xml")]);
    let drawing_xml = br#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <xdr:twoCellAnchor editAs="absolute">
    <xdr:from><xdr:col>1</xdr:col><xdr:colOff>267120</xdr:colOff><xdr:row>4</xdr:row><xdr:rowOff>69840</xdr:rowOff></xdr:from>
    <xdr:to><xdr:col>3</xdr:col><xdr:colOff>441720</xdr:colOff><xdr:row>7</xdr:row><xdr:rowOff>122040</xdr:rowOff></xdr:to>
    <xdr:grpSp>
      <xdr:nvGrpSpPr><xdr:cNvPr id="1" name=""/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr>
      <xdr:grpSpPr>
        <a:xfrm>
          <a:off x="1080000" y="720000"/><a:ext cx="1800000" cy="540000"/>
          <a:chOff x="1080000" y="720000"/><a:chExt cx="1800000" cy="540000"/>
        </a:xfrm>
      </xdr:grpSpPr>
      <xdr:pic>
        <xdr:nvPicPr><xdr:cNvPr id="2" name="Picture 1"/><xdr:cNvPicPr/></xdr:nvPicPr>
        <xdr:blipFill><a:blip r:embed="rIdEmbed1"/></xdr:blipFill>
        <xdr:spPr><a:xfrm><a:off x="1080000" y="720000"/><a:ext cx="720000" cy="360000"/></a:xfrm></xdr:spPr>
      </xdr:pic>
      <xdr:pic>
        <xdr:nvPicPr><xdr:cNvPr id="3" name="Picture 2"><a:hlinkClick r:id="rIdHyperlink"/></xdr:cNvPr><xdr:cNvPicPr/></xdr:nvPicPr>
        <xdr:blipFill><a:blip r:embed="rIdEmbed2"/></xdr:blipFill>
        <xdr:spPr><a:xfrm><a:off x="2160000" y="720000"/><a:ext cx="720000" cy="540000"/></a:xfrm></xdr:spPr>
      </xdr:pic>
    </xdr:grpSp>
    <xdr:clientData/>
  </xdr:twoCellAnchor>
</xdr:wsDr>"#;
    let drawing_rels: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdEmbed1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/>
  <Relationship Id="rIdEmbed2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image2.png"/>
  <Relationship Id="rIdHyperlink" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/second-logo" TargetMode="External"/>
</Relationships>"#;

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
        (
            "xl/worksheets/_rels/sheet1.xml.rels",
            worksheet_rels.as_bytes(),
        ),
        ("xl/drawings/drawing1.xml", drawing_xml),
        ("xl/drawings/_rels/drawing1.xml.rels", drawing_rels),
        // Content doesn't matter — xlsxparser never reads image bytes.
        ("xl/media/image1.png", b"\x89PNG\r\n\x1a\n" as &[u8]),
        ("xl/media/image2.png", b"\x89PNG\r\n\x1a\n" as &[u8]),
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
