//! Category 2 (異常系, `tests/fixtures/error/` in Issue #28): the library
//! must never panic on a malformed `.xlsx`, and must instead return the
//! matching `Error` variant.

use super::builder::*;

/// `xl/worksheets/sheet1.xml`'s closing tags are missing (truncated
/// mid-document) — a syntactic XML error. Verifies it surfaces as
/// `Error::XmlParse` rather than a panic.
pub fn corrupted_xml() -> Vec<u8> {
    // Deliberately missing `</row>`, `</sheetData>`, `</worksheet>`, and
    // even the final `>` of the dangling `<c` — quick-xml hits EOF with an
    // unterminated tag, which is unambiguously a syntax error (as opposed
    // to merely "well-formed but with an unbalanced tree", which some
    // non-validating parsers tolerate).
    let broken = br#"<worksheet><sheetData><row r="1"><c r="A1"><v>1</v></c><c r="B1"#;

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
        ("xl/worksheets/sheet1.xml", broken),
    ])
}

/// `xl/workbook.xml` references a sheet via `r:id="rId1"`, but
/// `xl/_rels/workbook.xml.rels` has no `Relationship` with that `Id` (only
/// the unrelated `styles` relationship) — simulates a corrupted or
/// incomplete rels part that points at a nonexistent sheet ID. Verifies it
/// surfaces as `Error::DanglingRelationship` rather than a panic.
pub fn missing_relations() -> Vec<u8> {
    let rows = r#"<row r="1"><c r="A1"><v>1</v></c></row>"#;

    build_zip(&[
        (
            "xl/_rels/workbook.xml.rels",
            // Note: no relationship for "rId1", the id workbook.xml's
            // <sheet> element references below.
            rels_xml(&[("rId2", "styles", "styles.xml")]).as_bytes(),
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

/// A cell's `<v>` references shared-string index 99999, but the SST
/// (`xl/sharedStrings.xml`) only has 1 entry. Verifies the out-of-bounds
/// index is rejected as `Error::SharedStringIndexOutOfBounds` rather than
/// panicking on the out-of-bounds access.
pub fn out_of_bounds_sst() -> Vec<u8> {
    let rows = r#"<row r="1"><c r="A1" t="s"><v>99999</v></c></row>"#;

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
            shared_strings_xml(&["only one entry"]).as_bytes(),
        ),
        (
            "xl/worksheets/sheet1.xml",
            worksheet_xml(rows, "").as_bytes(),
        ),
    ])
}

/// Two `<col>` ranges overlap (`1-5` and `3-8`). Verifies it surfaces as
/// `Error::InvalidColumnWidthRange` rather than silently registering both
/// or picking one arbitrarily (Issue #39).
pub fn overlapping_col_widths() -> Vec<u8> {
    let worksheet = r#"<worksheet>
<cols>
<col min="1" max="5" width="10"/>
<col min="3" max="8" width="20"/>
</cols>
<sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData>
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

/// A `<mergeCell ref="C3:A1"/>` has its start/end coordinates reversed
/// (end is above-and-left of start). Verifies the bounding-box computation
/// rejects it as `Error::InvalidMergedRange` rather than panicking or
/// silently producing a nonsensical region.
pub fn invalid_merge_ref() -> Vec<u8> {
    let rows = r#"<row r="1"><c r="A1"><v>1</v></c></row>"#;
    let merge_cells = r#"<mergeCells count="1"><mergeCell ref="C3:A1"/></mergeCells>"#;

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
