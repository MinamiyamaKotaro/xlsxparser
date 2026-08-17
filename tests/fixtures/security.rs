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
