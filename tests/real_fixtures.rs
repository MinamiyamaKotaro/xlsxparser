//! Integration tests for Categories 1 (正常系) and 3 (複合型) against
//! `.xlsx` files actually written by a real OOXML-writing tool (openpyxl —
//! see `scripts/generate_real_fixtures.py`), rather than the hand-authored
//! minimal XML in `tests/fixtures/*.rs`. Categories 2 (異常系) and 4 (負荷)
//! have the same "real file" treatment in `tests/real_error.rs` and
//! `tests/real_load.rs` respectively.
//!
//! These exist because hand-authored fixtures only ever exercise the
//! author's own understanding of the format; a genuine third-party writer
//! can differ in ways that matter, and did here — the first version of this
//! test caught a real bug: openpyxl writes worksheet relationship targets
//! as package-absolute paths (`Target="/xl/worksheets/sheet1.xml"`), which
//! `parse::relationships::resolve_target_path` mishandled (turning it into
//! the nonexistent `xl/xl/worksheets/sheet1.xml` and failing every sheet
//! with `Error::DanglingRelationship`) until it was fixed to special-case a
//! leading `/` per OPC (ECMA-376 Part 2).
//!
//! Regenerate the fixtures with `python3 scripts/generate_real_fixtures.py`
//! (requires `pip install openpyxl`) if they ever need to change.

use std::sync::Arc;
use xlsxparser::{parse_workbook, CellRef, CellValue, DateTimeValue, SheetVisibility};

fn fixture_path(relative: &str) -> String {
    format!("{}/tests/fixtures/{relative}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn real_basic_types_xlsx_maps_every_cell_type() {
    let workbook = parse_workbook(fixture_path("normal/basic_types.xlsx")).unwrap();
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
    // scripts/generate_real_fixtures.py's basic_types() writes D1 as
    // datetime.date(2023, 6, 15).
    assert_eq!(
        sheet.get(CellRef { row: 1, col: 4 }).unwrap().value,
        Some(CellValue::DateTime(DateTimeValue {
            year: 2023,
            month: 6,
            day: 15,
            hour: 0,
            minute: 0,
            second: 0,
        }))
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
}

#[test]
fn real_houganshi_merged_xlsx_resolves_the_whole_region_to_the_anchor() {
    let workbook = parse_workbook(fixture_path("complex/houganshi_merged.xlsx")).unwrap();
    let sheet = &workbook.sheets()[0];

    let anchor = sheet.get(CellRef { row: 1, col: 1 }).unwrap();
    assert_eq!(anchor.value, Some(CellValue::Text(Arc::from("houganshi"))));

    for row in 1..=3 {
        for col in 1..=3 {
            assert_eq!(
                sheet.get(CellRef { row, col }),
                sheet.get(CellRef { row: 1, col: 1 })
            );
        }
    }
}

#[test]
fn real_multi_sheet_states_xlsx_enumerates_every_visibility() {
    let workbook = parse_workbook(fixture_path("complex/multi_sheet_states.xlsx")).unwrap();
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
    assert_eq!(by_name["Empty"].iter_cells().count(), 0);
}

#[test]
fn real_embedded_image_xlsx_resolves_anchor_embed_and_hyperlink() {
    use xlsxparser::ImageAnchor;

    let workbook = parse_workbook(fixture_path("complex/embedded_image.xlsx")).unwrap();
    let sheet = &workbook.sheets()[0];

    let images = sheet.images();
    assert_eq!(images.len(), 1);
    let image = &images[0];

    assert_eq!(image.target, "xl/media/image1.png");
    assert_eq!(
        image.hyperlink.as_deref(),
        Some("https://example.com/sample-image")
    );
    // scripts/generate_real_fixtures.py's embedded_image() anchors
    // B2:E9 with a 10000/20000 EMU offset on the origin corner — 0-based
    // xdr:col=1/xdr:row=1 becomes 1-based CellRef { row: 2, col: 2 }.
    match image.anchor {
        ImageAnchor::TwoCell { from, to } => {
            assert_eq!(from.cell, CellRef { row: 2, col: 2 });
            assert_eq!(from.col_off, 10000);
            assert_eq!(from.row_off, 20000);
            assert_eq!(to.cell, CellRef { row: 9, col: 5 });
        }
        ImageAnchor::OneCell { .. } => panic!("expected a TwoCell anchor"),
    }

    // The embedded media's real bytes are never read into the model.
    let json = xlsxparser::to_json_string(&workbook).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let json_image = &parsed["sheets"][0]["images"][0];
    assert_eq!(json_image["target"], "xl/media/image1.png");
    assert_eq!(json_image["anchor"]["type"], "twoCell");
}

#[test]
fn real_extreme_sparse_xlsx_registers_only_the_two_populated_cells() {
    let workbook = parse_workbook(fixture_path("complex/extreme_sparse.xlsx")).unwrap();
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
    assert_eq!(sheet.iter_cells().count(), 2);
}
