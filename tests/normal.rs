//! Integration tests for Category 1 (正常系) fixtures — see
//! `tests/fixtures/normal.rs` for what each package under test looks like.

#[path = "fixtures/mod.rs"]
mod fixtures;

use fixtures::normal;
use std::io::Cursor;
use std::sync::Arc;
use xlsxparser::{parse_workbook_reader, CellRef, CellValue, DateTimeValue};

#[test]
fn basic_types_maps_each_cell_to_the_right_json_type() {
    let workbook = parse_workbook_reader(Cursor::new(normal::basic_types())).unwrap();
    let sheet = &workbook.sheets()[0];

    assert_eq!(
        sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
        Some(CellValue::Text(Arc::from("日本語Text")))
    );
    assert_eq!(
        sheet.get(CellRef { row: 1, col: 2 }).unwrap().value,
        Some(CellValue::Number(42.0))
    );
    assert_eq!(
        sheet.get(CellRef { row: 1, col: 3 }).unwrap().value,
        Some(CellValue::Number(19.99))
    );
    assert_eq!(
        sheet.get(CellRef { row: 1, col: 4 }).unwrap().value,
        Some(CellValue::DateTime(DateTimeValue))
    );
    assert_eq!(
        sheet.get(CellRef { row: 1, col: 5 }).unwrap().value,
        Some(CellValue::Boolean(true))
    );
    assert_eq!(
        sheet.get(CellRef { row: 1, col: 6 }).unwrap().value,
        Some(CellValue::Boolean(false))
    );
    assert_eq!(
        sheet.get(CellRef { row: 1, col: 7 }).unwrap().value,
        Some(CellValue::Error("#N/A".to_string()))
    );

    // No blank cell was ever instantiated for an untouched coordinate
    // (e.g. H1) — the sparse model only holds what was actually populated.
    assert_eq!(sheet.iter_cells().count(), 7);

    let json = xlsxparser::to_json_string(&workbook).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let cells = parsed["sheets"][0]["cells"].as_array().unwrap();
    let mut types: Vec<&str> = cells
        .iter()
        .map(|c| c["value"]["type"].as_str().unwrap())
        .collect();
    types.sort_unstable();
    // Sheet::iter_cells (HashMap-backed) makes no order guarantee, so this
    // compares the type multiset rather than positional order. The date
    // cell (D1) is present here as "empty", not "dateTime": `DateTimeValue`
    // is currently a data-less placeholder, so `json.rs::cell_value_to_json`
    // always falls back to `Empty` for it (see that module's doc comment) —
    // this is today's documented behavior, not a fixture bug.
    let mut expected = vec![
        "text", "number", "number", "empty", "boolean", "boolean", "error",
    ];
    expected.sort_unstable();
    assert_eq!(types, expected);
}

#[test]
fn shared_strings_resolves_repeated_and_distinct_indices() {
    let workbook = parse_workbook_reader(Cursor::new(normal::shared_strings())).unwrap();
    let sheet = &workbook.sheets()[0];

    assert_eq!(
        sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
        Some(CellValue::Text(Arc::from("Apple")))
    );
    assert_eq!(
        sheet.get(CellRef { row: 1, col: 2 }).unwrap().value,
        Some(CellValue::Text(Arc::from("Banana")))
    );
    // Same SST entry (index 0) referenced from a second cell resolves to
    // the same text.
    assert_eq!(
        sheet.get(CellRef { row: 1, col: 3 }).unwrap().value,
        Some(CellValue::Text(Arc::from("Apple")))
    );
}

#[test]
fn inline_strings_are_extracted_without_a_shared_string_table() {
    let workbook = parse_workbook_reader(Cursor::new(normal::inline_strings())).unwrap();
    let sheet = &workbook.sheets()[0];

    assert_eq!(
        sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
        Some(CellValue::Text(Arc::from("Inline One")))
    );
    assert_eq!(
        sheet.get(CellRef { row: 1, col: 2 }).unwrap().value,
        Some(CellValue::Text(Arc::from("Inline Two")))
    );
}
