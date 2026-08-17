//! Integration tests for Category 4 (負荷) fixtures — see
//! `tests/fixtures/load.rs` for how each package is generated. These are
//! slower than the rest of the suite by design (hundreds of thousands of
//! cells / thousands of sheet entries); `cargo test --test load` runs just
//! this file in isolation.

#[path = "fixtures/mod.rs"]
mod fixtures;

use fixtures::load;
use std::io::Cursor;
use xlsxparser::{parse_workbook_reader, CellRef, CellValue};

#[test]
fn massive_dense_accounting_parses_every_one_of_300_000_cells() {
    let workbook = parse_workbook_reader(Cursor::new(load::massive_dense_accounting())).unwrap();
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
fn thousand_sheets_all_parse_without_exhausting_resources() {
    let workbook = parse_workbook_reader(Cursor::new(load::thousand_sheets())).unwrap();
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
fn massive_sst_resolves_indices_across_a_50_000_entry_table() {
    let workbook = parse_workbook_reader(Cursor::new(load::massive_sst())).unwrap();
    let sheet = &workbook.sheets()[0];

    assert_eq!(
        sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
        Some(CellValue::Text(std::sync::Arc::from("unique-string-0")))
    );
    assert_eq!(
        sheet.get(CellRef { row: 1, col: 2 }).unwrap().value,
        Some(CellValue::Text(std::sync::Arc::from("unique-string-25000")))
    );
    assert_eq!(
        sheet.get(CellRef { row: 1, col: 3 }).unwrap().value,
        Some(CellValue::Text(std::sync::Arc::from("unique-string-49999")))
    );
}
