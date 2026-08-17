//! Integration tests for Category 4 (負荷) against real, openpyxl-generated
//! `.xlsx` files — see `scripts/generate_real_fixtures.py`'s
//! `massive_dense_accounting`/`thousand_sheets`/`massive_sst`. Complements
//! `tests/load.rs` (Rust-builder-generated equivalents); this file proves
//! the same scale holds up against files an actual writer produces, at the
//! cost of being slower and needing binary fixtures on disk, so it's kept
//! separate like `tests/load.rs` is.

use std::sync::Arc;
use xlsxparser::{parse_workbook, CellRef, CellValue};

fn fixture_path(relative: &str) -> String {
    format!("{}/tests/fixtures/{relative}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn real_massive_dense_accounting_parses_every_one_of_300_000_cells() {
    let workbook = parse_workbook(fixture_path("load/massive_dense_accounting.xlsx")).unwrap();
    let sheet = &workbook.sheets()[0];

    assert_eq!(sheet.iter_cells().count(), 10_000 * 30);
    assert_eq!(
        sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
        Some(CellValue::Number(101.0))
    );
    assert_eq!(
        sheet
            .get(CellRef {
                row: 10_000,
                col: 30
            })
            .unwrap()
            .value,
        Some(CellValue::Number(1_000_030.0))
    );
}

#[test]
fn real_thousand_sheets_all_parse_without_exhausting_resources() {
    let workbook = parse_workbook(fixture_path("load/thousand_sheets.xlsx")).unwrap();
    let sheets = workbook.sheets();

    assert_eq!(sheets.len(), 1000);
    assert_eq!(sheets[0].name, "Sheet1");
    assert_eq!(sheets[999].name, "Sheet1000");
    assert_eq!(
        sheets[499].get(CellRef { row: 1, col: 1 }).unwrap().value,
        Some(CellValue::Number(500.0))
    );
}

#[test]
fn real_massive_sst_resolves_indices_across_a_50_000_entry_table() {
    let workbook = parse_workbook(fixture_path("load/massive_sst.xlsx")).unwrap();
    let sheet = &workbook.sheets()[0];

    assert_eq!(
        sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
        Some(CellValue::Text(Arc::from("unique-string-0")))
    );
    assert_eq!(
        sheet.get(CellRef { row: 1, col: 2 }).unwrap().value,
        Some(CellValue::Text(Arc::from("unique-string-25000")))
    );
    assert_eq!(
        sheet.get(CellRef { row: 1, col: 3 }).unwrap().value,
        Some(CellValue::Text(Arc::from("unique-string-49999")))
    );
}
