//! Integration tests for Category 1 (正常系) fixtures — see
//! `tests/fixtures/normal.rs` for what each package under test looks like.

#[path = "fixtures/mod.rs"]
mod fixtures;

use fixtures::normal;
use std::io::Cursor;
use std::sync::Arc;
use xlsxparser::{parse_workbook_reader, to_json_string, CellRef, CellValue, DateTimeValue};

#[test]
fn wrap_text_resolves_per_cell_and_serializes_nested_under_style() {
    let workbook = parse_workbook_reader(Cursor::new(normal::wrap_text_styles())).unwrap();
    let sheet = &workbook.sheets()[0];

    assert!(
        sheet
            .get(CellRef { row: 1, col: 1 })
            .unwrap()
            .style
            .as_ref()
            .unwrap()
            .wrap_text
    );
    assert!(
        !sheet
            .get(CellRef { row: 1, col: 2 })
            .unwrap()
            .style
            .as_ref()
            .unwrap()
            .wrap_text
    );

    let json = to_json_string(&workbook).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let cells = parsed["sheets"][0]["cells"].as_array().unwrap();
    let a1 = cells.iter().find(|c| c["col"] == 1).unwrap();
    let b1 = cells.iter().find(|c| c["col"] == 2).unwrap();
    assert_eq!(a1["style"]["wrapText"], serde_json::json!(true));
    assert_eq!(b1["style"]["wrapText"], serde_json::json!(false));
}

#[test]
fn font_size_and_bold_resolve_per_cell_and_serialize_nested_under_style() {
    let workbook = parse_workbook_reader(Cursor::new(normal::font_styles())).unwrap();
    let sheet = &workbook.sheets()[0];

    let a1_font = &sheet
        .get(CellRef { row: 1, col: 1 })
        .unwrap()
        .style
        .as_ref()
        .unwrap()
        .font;
    assert_eq!(a1_font.size_pt, 11.0);
    assert!(!a1_font.bold);

    let b1_font = &sheet
        .get(CellRef { row: 1, col: 2 })
        .unwrap()
        .style
        .as_ref()
        .unwrap()
        .font;
    assert_eq!(b1_font.size_pt, 14.0);
    assert!(b1_font.bold);

    let json = to_json_string(&workbook).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let cells = parsed["sheets"][0]["cells"].as_array().unwrap();
    let a1 = cells.iter().find(|c| c["col"] == 1).unwrap();
    let b1 = cells.iter().find(|c| c["col"] == 2).unwrap();
    assert_eq!(
        a1["style"],
        serde_json::json!({
            "font": { "sizePt": 11.0, "bold": false },
            "wrapText": false
        })
    );
    assert_eq!(
        b1["style"],
        serde_json::json!({
            "font": { "sizePt": 14.0, "bold": true },
            "wrapText": false
        })
    );
}

#[test]
fn column_widths_resolve_per_column_and_serialize_as_a_sheet_level_array() {
    let workbook = parse_workbook_reader(Cursor::new(normal::column_widths())).unwrap();
    let sheet = &workbook.sheets()[0];

    assert_eq!(sheet.column_width(1), Some(12.5)); // in the A-C range
    assert_eq!(sheet.column_width(3), Some(12.5));
    assert_eq!(sheet.column_width(4), Some(9.1)); // gap -> defaultColWidth
    assert_eq!(sheet.column_width(5), Some(30.0)); // the E-only range
    assert_eq!(sheet.column_width(6), Some(9.1)); // beyond every range -> default

    let json = to_json_string(&workbook).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let sheet_json = &parsed["sheets"][0];
    assert_eq!(sheet_json["defaultColumnWidth"], serde_json::json!(9.1));
    assert_eq!(
        sheet_json["columns"],
        serde_json::json!([
            {"min": 1, "max": 3, "width": 12.5},
            {"min": 5, "max": 5, "width": 30.0}
        ])
    );
    // Not duplicated onto individual cells.
    for cell in sheet_json["cells"].as_array().unwrap() {
        assert!(cell.get("columnWidth").is_none());
    }
}

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
