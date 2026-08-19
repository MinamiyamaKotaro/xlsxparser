//! Integration tests for Category 3 (正常系(複合型)) fixtures — see
//! `tests/fixtures/complex.rs` for the packages under test.

#[path = "fixtures/mod.rs"]
mod fixtures;

use fixtures::complex;
use std::io::Cursor;
use std::sync::Arc;
use xlsxparser::{parse_workbook_reader, CellRef, CellValue, SheetVisibility};

#[test]
fn houganshi_merged_region_resolves_every_coordinate_to_the_anchor() {
    let workbook = parse_workbook_reader(Cursor::new(complex::houganshi_merged())).unwrap();
    let sheet = &workbook.sheets()[0];

    let anchor = sheet.get(CellRef { row: 1, col: 1 }).unwrap();
    assert_eq!(anchor.value, Some(CellValue::Text(Arc::from("houganshi"))));

    // Every coordinate inside A1:C3 resolves to the same cell as the
    // anchor, including the opposite corner C3.
    for row in 1..=3 {
        for col in 1..=3 {
            assert_eq!(
                sheet.get(CellRef { row, col }),
                sheet.get(CellRef { row: 1, col: 1 }),
                "coordinate ({row}, {col}) did not resolve to the merge anchor"
            );
        }
    }

    let json = xlsxparser::to_json_string(&workbook).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let cells = parsed["sheets"][0]["cells"].as_array().unwrap();
    // The 9 virtual coordinates never appear as separate JSON cells.
    assert_eq!(cells.len(), 1);
    assert_eq!(cells[0]["rowSpan"], 3);
    assert_eq!(cells[0]["colSpan"], 3);
}

#[test]
fn multi_sheet_states_are_all_enumerated_including_hidden_and_empty() {
    let workbook = parse_workbook_reader(Cursor::new(complex::multi_sheet_states())).unwrap();
    let sheets = workbook.sheets();

    assert_eq!(sheets.len(), 4);
    let by_name: std::collections::HashMap<&str, &xlsxparser::Sheet> =
        sheets.iter().map(|s| (s.name.as_str(), s)).collect();

    assert_eq!(by_name["Visible"].visibility, SheetVisibility::Visible);
    assert_eq!(by_name["Hidden"].visibility, SheetVisibility::Hidden);
    assert_eq!(
        by_name["VeryHidden"].visibility,
        SheetVisibility::VeryHidden
    );
    assert_eq!(by_name["Empty"].visibility, SheetVisibility::Visible);
    assert_eq!(by_name["Empty"].iter_cells().count(), 0);

    // All 4 sheets are enumerated in the JSON output too, none dropped.
    let json = xlsxparser::to_json_string(&workbook).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["sheets"].as_array().unwrap().len(), 4);
}

#[test]
fn embedded_image_resolves_anchor_embed_and_hyperlink() {
    use xlsxparser::ImageAnchor;

    let workbook = parse_workbook_reader(Cursor::new(complex::embedded_image())).unwrap();
    let sheet = &workbook.sheets()[0];

    let images = sheet.images();
    assert_eq!(images.len(), 1);
    let image = &images[0];

    assert_eq!(image.target, "xl/media/image1.png");
    assert_eq!(
        image.hyperlink.as_deref(),
        Some("https://example.com/sample-image")
    );
    match image.anchor {
        ImageAnchor::TwoCell { from, to } => {
            assert_eq!(from.cell, CellRef { row: 2, col: 2 });
            assert_eq!(from.col_off, 10000);
            assert_eq!(from.row_off, 20000);
            assert_eq!(to.cell, CellRef { row: 9, col: 5 });
            assert_eq!(to.col_off, 0);
            assert_eq!(to.row_off, 0);
        }
        ImageAnchor::OneCell { .. } => panic!("expected a TwoCell anchor"),
    }

    let json = xlsxparser::to_json_string(&workbook).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let json_image = &parsed["sheets"][0]["images"][0];
    assert_eq!(json_image["anchor"]["type"], "twoCell");
    assert_eq!(json_image["anchor"]["from"]["row"], 2);
    assert_eq!(json_image["anchor"]["from"]["col"], 2);
    assert_eq!(json_image["target"], "xl/media/image1.png");
    assert_eq!(json_image["hyperlink"], "https://example.com/sample-image");
}

#[test]
fn extreme_sparse_coordinates_register_only_the_populated_cells() {
    let workbook = parse_workbook_reader(Cursor::new(complex::extreme_sparse())).unwrap();
    let sheet = &workbook.sheets()[0];

    assert_eq!(
        sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
        Some(CellValue::Number(1.0))
    );
    assert_eq!(
        sheet
            .get(CellRef {
                row: 1_048_576,
                col: 16_384
            })
            .unwrap()
            .value,
        Some(CellValue::Number(2.0))
    );
    // Only the 2 populated coordinates exist — the ~1.7 trillion cells in
    // between were never touched.
    assert_eq!(sheet.iter_cells().count(), 2);
}
