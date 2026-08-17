//! Category 4 (負荷, `tests/fixtures/load/` in Issue #28): stress tests for
//! the README's "fast, low-memory" claim. Per the issue's own
//! recommendation, these are generated at test time rather than committed
//! as binary fixtures, since they would otherwise be impractically large
//! to keep in the repository.

use super::builder::*;
use std::fmt::Write as _;

/// A dense 10,000-row x 30-column sheet (300,000 populated cells) — an
/// accounting-ledger-shaped worst case for the sparse-storage design (no
/// gaps to skip). Verifies parsing completes and every cell is present,
/// without asserting on wall-clock time (CI hardware varies too much for a
/// hard timing assertion to be reliable; `cargo test`'s own wall time
/// serves as a smoke signal that this remains practical).
pub fn massive_dense_accounting() -> Vec<u8> {
    const ROWS: u32 = 10_000;
    const COLS: u32 = 30;

    let mut rows_xml = String::with_capacity((ROWS * COLS * 24) as usize);
    for row in 1..=ROWS {
        rows_xml.push_str(&format!("<row r=\"{row}\">"));
        for col in 1..=COLS {
            let col_letters = column_letters(col);
            let value = row as f64 * 100.0 + col as f64;
            let _ = write!(rows_xml, "<c r=\"{col_letters}{row}\"><v>{value}</v></c>");
        }
        rows_xml.push_str("</row>\n");
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
            workbook_xml(&[("Ledger", "rId1", None)]).as_bytes(),
        ),
        ("xl/styles.xml", DEFAULT_STYLES_XML),
        (
            "xl/worksheets/sheet1.xml",
            worksheet_xml(&rows_xml, "").as_bytes(),
        ),
    ])
}

/// 1,000 sheets, each holding a single cell. Verifies the pipeline streams
/// through the whole workbook (opening/parsing each worksheet part in turn,
/// dropping its buffers before moving to the next — see `pipeline.rs`'s
/// per-sheet loop) without needing to keep every sheet's ZIP entry or
/// worksheet buffer alive simultaneously.
pub fn thousand_sheets() -> Vec<u8> {
    const SHEET_COUNT: u32 = 1000;

    let mut entries: Vec<(String, Vec<u8>)> = Vec::with_capacity(SHEET_COUNT as usize + 3);

    let mut relationships: Vec<(String, String, String)> =
        Vec::with_capacity(SHEET_COUNT as usize + 1);
    let mut sheets: Vec<(String, String, Option<String>)> =
        Vec::with_capacity(SHEET_COUNT as usize);
    for i in 1..=SHEET_COUNT {
        let r_id = format!("rId{i}");
        relationships.push((
            r_id.clone(),
            "worksheet".to_string(),
            format!("worksheets/sheet{i}.xml"),
        ));
        sheets.push((format!("Sheet{i}"), r_id, None));

        let cell = format!("<row r=\"1\"><c r=\"A1\"><v>{i}</v></c></row>");
        entries.push((
            format!("xl/worksheets/sheet{i}.xml"),
            worksheet_xml(&cell, "").into_bytes(),
        ));
    }
    relationships.push((
        "rIdStyles".to_string(),
        "styles".to_string(),
        "styles.xml".to_string(),
    ));

    let rel_refs: Vec<(&str, &str, &str)> = relationships
        .iter()
        .map(|(id, ty, target)| (id.as_str(), ty.as_str(), target.as_str()))
        .collect();
    let sheet_refs: Vec<(&str, &str, Option<&str>)> = sheets
        .iter()
        .map(|(name, r_id, state)| (name.as_str(), r_id.as_str(), state.as_deref()))
        .collect();

    entries.push((
        "xl/_rels/workbook.xml.rels".to_string(),
        rels_xml(&rel_refs).into_bytes(),
    ));
    entries.push((
        "xl/workbook.xml".to_string(),
        workbook_xml(&sheet_refs).into_bytes(),
    ));
    entries.push(("xl/styles.xml".to_string(), DEFAULT_STYLES_XML.to_vec()));

    build_zip_owned(&entries)
}

/// A Shared String Table with 50,000 unique entries (every string distinct,
/// so none of them collapse via dedup) — the on-memory-SST worst case.
/// Cells reference the first, a middle, and the last index. Verifies a
/// large SST parses correctly and indices resolve without an OOM, without
/// actually growing the fixture into the hundreds-of-thousands range Excel
/// itself could reach (50,000 already exercises the same code path; this
/// stays well clear of the 512 MiB per-entry Zip Bomb cap so the test
/// isn't implicitly exercising that unrelated limit).
pub fn massive_sst() -> Vec<u8> {
    const STRING_COUNT: usize = 50_000;

    let strings: Vec<String> = (0..STRING_COUNT)
        .map(|i| format!("unique-string-{i}"))
        .collect();
    let string_refs: Vec<&str> = strings.iter().map(String::as_str).collect();

    let rows = format!(
        "<row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c><c r=\"B1\" t=\"s\"><v>{mid}</v></c><c r=\"C1\" t=\"s\"><v>{last}</v></c></row>",
        mid = STRING_COUNT / 2,
        last = STRING_COUNT - 1,
    );

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
            shared_strings_xml(&string_refs).as_bytes(),
        ),
        (
            "xl/worksheets/sheet1.xml",
            worksheet_xml(&rows, "").as_bytes(),
        ),
    ])
}

/// Converts a 1-based column number to A1-style letters (mirrors
/// `model::cell`'s private `column_number_to_letters`, duplicated here
/// since that helper isn't part of the public API fixtures can depend on).
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
