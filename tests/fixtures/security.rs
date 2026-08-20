//! Category 5 (セキュリティ, `tests/fixtures/security/` in Issue #28):
//! proves the sanitization layer described in `docs/security/design-review.md`
//! actually stops each attack, rather than merely existing in
//! `container/sanitize.rs` unit tests that never touch a real archive.
//!
//! These fixtures carry the *mechanism* of each attack (a high-compression
//! payload, a traversal path, a DOCTYPE) without also being the kind of
//! opaque binary blob that's unreviewable in a diff — see `builder.rs`'s
//! module doc.

use super::builder::*;

/// `xl/worksheets/sheet1.xml`'s content is 20,000,000 bytes of a single
/// repeated character — a payload chosen specifically because `Deflate`
/// compresses it to a few KB, i.e. a large compression ratio, while still
/// being cheap for *this test* to generate and zip up. The accompanying
/// test in `tests/security.rs` parses this with a caller-supplied
/// `SizeLimits::max_entry_size` set well below 20,000,000 (rather than the
/// crate's 512 MiB default) purely so the test runs in milliseconds
/// instead of needing a payload that actually exceeds 512 MiB — the
/// detection mechanism under test (`BoundedReader` counting real bytes
/// read, never trusting the ZIP header's declared size) is exactly the
/// same regardless of which cap value trips it.
pub fn zip_bomb() -> Vec<u8> {
    let mut sheet = String::with_capacity(20_000_048);
    sheet.push_str("<worksheet><sheetData><row r=\"1\"><c r=\"A1\" t=\"str\"><v>");
    sheet.extend(std::iter::repeat_n('A', 20_000_000));
    sheet.push_str("</v></c></row></sheetData></worksheet>");

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
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
    ])
}

/// 20,000 single-cell merges (at `MAX_MERGE_REGIONS`, `resolve::merge`'s
/// cap) arranged so their combined bounding box covers virtually the whole
/// sheet: most sit on row 2, but one is planted at the sheet's far corner
/// (`XFD1048576`) purely to stretch `merge_bounds`. 300,000 *distinct* data
/// cells sit far from every merge (rows 500,000 onward, wrapping to a new
/// row every 16,384 columns so each one gets its own coordinate — a fixed
/// row here would collapse them all into at most 16,384 unique `CellRef`s,
/// silently understating both the fixture's cell count and any benchmark
/// run against it), each carrying a non-default style (`s="0"`) so a
/// `PendingStyle` gets recorded for it — a structurally valid file (every
/// merge is 1x1, non-overlapping, and within `MAX_MERGE_REGIONS`) rather
/// than a malformed one.
///
/// Before `Sheet::finalize_merges` (Issue #43), `json.rs`'s `iter_cells`
/// resolved each of those 300,000 cells by scanning all 20,000 merged
/// regions (the O(1) `merge_bounds` pre-check never rejects them, since
/// they fall inside it) — a directly measured multi-second CPU stall from
/// a file only a few hundred KB in size, with none of the individual
/// merge-count/cell-count/byte-size limits actually violated.
pub fn sparse_merge_bounding_box_amplification() -> Vec<u8> {
    const NUM_FILLER_MERGES: usize = 19_999;
    const NUM_DATA_CELLS: usize = 300_000;

    let mut merges = String::new();
    for i in 0..NUM_FILLER_MERGES {
        let col = (i % 16_384) + 1;
        let row = 2 + (i / 16_384);
        let a1 = format!("{}{row}", column_letters(col as u32));
        merges.push_str(&format!("<mergeCell ref=\"{a1}:{a1}\"/>"));
    }
    merges.push_str("<mergeCell ref=\"XFD1048576:XFD1048576\"/>");
    let merge_cells_xml = format!(
        "<mergeCells count=\"{}\">{merges}</mergeCells>",
        NUM_FILLER_MERGES + 1
    );

    let mut rows = String::new();
    for i in 0..NUM_DATA_CELLS {
        let col = (i % 16_384) + 1;
        let row = 500_000 + (i / 16_384);
        rows.push_str(&format!(
            "<row r=\"{row}\"><c r=\"{}{row}\" s=\"0\"><v>1</v></c></row>\n",
            column_letters(col as u32)
        ));
    }

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
            worksheet_xml(&rows, &merge_cells_xml).as_bytes(),
        ),
    ])
}

/// Converts a 1-based column number to A1-style letters (mirrors
/// `model::cell`'s private `column_number_to_letters`; duplicated here for
/// the same reason `tests/fixtures/load.rs` does).
fn column_letters(mut n: u32) -> String {
    let mut buf = Vec::new();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        buf.push(b'A' + rem);
        n = (n - 1) / 26;
    }
    buf.reverse();
    String::from_utf8(buf).unwrap()
}

/// An otherwise-normal package that also contains one ZIP entry named
/// `../../../../../../../tmp/evil.txt` — a path-traversal entry name of the
/// kind Zip Slip exploits to write outside the intended extraction
/// directory. Verifies `ZipContainer::open_reader` rejects the *whole*
/// archive (`Error::ZipSlipDetected`) the moment it enumerates a single bad
/// entry name, before any part is ever read.
pub fn zip_slip() -> Vec<u8> {
    let rows = r#"<row r="1"><c r="A1"><v>1</v></c></row>"#;

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
        (
            "../../../../../../../tmp/evil.txt",
            b"planted by a Zip Slip attack",
        ),
    ])
}

/// `xl/workbook.xml` carries a `<!DOCTYPE>` declaration with an external
/// entity that, if expanded, would attempt to read `/etc/passwd`. Verifies
/// `read_event`'s fail-closed check rejects the DOCTYPE's mere presence
/// (`Error::DoctypeRejected`) without ever attempting entity expansion.
pub fn xxe_attack() -> Vec<u8> {
    let malicious_workbook = br#"<?xml version="1.0"?>
<!DOCTYPE workbook [<!ENTITY xxe SYSTEM "file:///etc/passwd">]>
<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
    <sheet name="Sheet1" sheetId="1" r:id="rId1"/>
  </sheets>
</workbook>"#;
    let rows = r#"<row r="1"><c r="A1"><v>1</v></c></row>"#;

    build_zip(&[
        (
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[
                ("rId1", "worksheet", "worksheets/sheet1.xml"),
                ("rId2", "styles", "styles.xml"),
            ])
            .as_bytes(),
        ),
        ("xl/workbook.xml", malicious_workbook),
        ("xl/styles.xml", DEFAULT_STYLES_XML),
        (
            "xl/worksheets/sheet1.xml",
            worksheet_xml(rows, "").as_bytes(),
        ),
    ])
}

/// `n_cells` populated cells (`<c r="..."><v>1</v></c>`, each carrying a
/// value so it actually reaches `Sheet::insert_cell` rather than being
/// dropped by `flush_cell` for carrying no value/style/shared-string
/// reference — see `SizeLimits::max_cells_per_sheet`'s doc comment). Used
/// with a caller-supplied tiny `max_cells_per_sheet` (same technique as
/// [`zip_bomb`]'s caller-supplied tiny `max_entry_size`) so the accompanying
/// test in `tests/security.rs` runs in milliseconds instead of needing a
/// fixture anywhere near the real 5,000,000-cell default cap.
pub fn too_many_cells(n_cells: u32) -> Vec<u8> {
    let mut rows = String::new();
    for col in 1..=n_cells {
        rows.push_str(&format!(
            "<row r=\"{col}\"><c r=\"A{col}\"><v>1</v></c></row>\n"
        ));
    }

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
            worksheet_xml(&rows, "").as_bytes(),
        ),
    ])
}

/// Same as [`too_many_cells`], but each cell is a self-closing `<c r="..."
/// s="0"/>` (a style attribute, no `<v>` — the shape `<row>`/`<c>` take when
/// self-closing, which only reaches `Sheet::insert_cell` if it carries a
/// style, unlike a bare `<c r="A1"/>`). Exercises the `is_empty` branch of
/// `parse/worksheet.rs`'s `<c>` handling — as opposed to [`too_many_cells`],
/// which only ever produces non-self-closing `<c>...</c>` cells.
pub fn too_many_cells_self_closing_with_style(n_cells: u32) -> Vec<u8> {
    let mut rows = String::new();
    for col in 1..=n_cells {
        rows.push_str(&format!(
            "<row r=\"{col}\"><c r=\"A{col}\" s=\"0\"/></row>\n"
        ));
    }

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
            worksheet_xml(&rows, "").as_bytes(),
        ),
    ])
}
