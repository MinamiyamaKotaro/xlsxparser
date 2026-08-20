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
fn embedded_image_one_cell_resolves_a_single_cell_confined_anchor() {
    use xlsxparser::{AnchorMarker, ImageAnchor, ImageExtent};

    let workbook = parse_workbook_reader(Cursor::new(complex::embedded_image_one_cell())).unwrap();
    let sheet = &workbook.sheets()[0];

    let images = sheet.images();
    assert_eq!(images.len(), 1);
    let image = &images[0];

    assert_eq!(image.target, "xl/media/image1.png");
    assert_eq!(image.hyperlink, None);
    assert_eq!(
        image.anchor,
        ImageAnchor::OneCell {
            from: AnchorMarker {
                cell: CellRef { row: 5, col: 3 },
                col_off: 5000,
                row_off: 5000,
            },
            ext: ImageExtent {
                cx: 400_000,
                cy: 150_000,
            },
        }
    );

    let json = xlsxparser::to_json_string(&workbook).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let json_image = &parsed["sheets"][0]["images"][0];
    assert_eq!(json_image["anchor"]["type"], "oneCell");
    assert_eq!(json_image["anchor"]["from"]["row"], 5);
    assert_eq!(json_image["anchor"]["from"]["col"], 3);
    assert_eq!(json_image["anchor"]["ext"]["cx"], 400_000);
    assert_eq!(json_image["anchor"]["ext"]["cy"], 150_000);
    // to/hyperlink are both absent for a hyperlink-less OneCell anchor.
    assert!(json_image["anchor"].get("to").is_none());
    assert!(json_image.get("hyperlink").is_none());
}

#[test]
fn grouped_images_resolve_relative_to_group_with_per_pic_hyperlink_scoping() {
    use xlsxparser::{AnchorMarker, ImageAnchor, ImageExtent};

    let workbook = parse_workbook_reader(Cursor::new(complex::grouped_images())).unwrap();
    let sheet = &workbook.sheets()[0];

    let images = sheet.images();
    assert_eq!(images.len(), 2);

    assert_eq!(images[0].target, "xl/media/image1.png");
    assert_eq!(images[0].hyperlink, None);
    assert_eq!(
        images[0].anchor,
        ImageAnchor::OneCell {
            from: AnchorMarker {
                cell: CellRef { row: 5, col: 2 },
                col_off: 267_120,
                row_off: 69_840,
            },
            ext: ImageExtent {
                cx: 720_000,
                cy: 360_000,
            },
        }
    );

    assert_eq!(images[1].target, "xl/media/image2.png");
    assert_eq!(
        images[1].hyperlink.as_deref(),
        Some("https://example.com/second-logo")
    );
    // delta = 2_160_000 - 1_080_000 = 1_080_000, added to the anchor's own colOff.
    assert_eq!(
        images[1].anchor,
        ImageAnchor::OneCell {
            from: AnchorMarker {
                cell: CellRef { row: 5, col: 2 },
                col_off: 267_120 + 1_080_000,
                row_off: 69_840,
            },
            ext: ImageExtent {
                cx: 720_000,
                cy: 540_000,
            },
        }
    );

    let json = xlsxparser::to_json_string(&workbook).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let json_images = parsed["sheets"][0]["images"].as_array().unwrap();
    assert_eq!(json_images.len(), 2);
    assert_eq!(json_images[0]["anchor"]["type"], "oneCell");
    assert!(json_images[0].get("hyperlink").is_none());
    assert_eq!(json_images[1]["target"], "xl/media/image2.png");
    assert_eq!(
        json_images[1]["hyperlink"],
        "https://example.com/second-logo"
    );
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

#[test]
fn medium_density_registers_only_the_scattered_populated_cells() {
    // tests/README.md edge-case audit: a tier between extreme_sparse (2
    // cells) and massive_dense_accounting (100% fill in a fixed
    // rectangle) — cells scattered at partial density across a wide
    // range, with genuine gaps around any given populated cell.
    let workbook = parse_workbook_reader(Cursor::new(complex::medium_density())).unwrap();
    let sheet = &workbook.sheets()[0];

    let expected_count = (1..=complex::MEDIUM_DENSITY_ROWS)
        .flat_map(|row| (1..=complex::MEDIUM_DENSITY_COLS).map(move |col| (row, col)))
        .filter(|(row, col)| (row + col).is_multiple_of(10))
        .count();
    assert_eq!(sheet.iter_cells().count(), expected_count);

    // Spot-check a populated cell and confirm its immediate neighbor
    // (unpopulated under the (row + col) % 10 == 0 rule) is genuinely
    // absent, not merely zero-valued.
    assert_eq!(
        sheet.get(CellRef { row: 1, col: 9 }).unwrap().value,
        Some(CellValue::Number(1009.0))
    );
    assert!(sheet.get(CellRef { row: 1, col: 8 }).is_none());
    assert!(sheet.get(CellRef { row: 1, col: 10 }).is_none());

    let last_row = complex::MEDIUM_DENSITY_ROWS;
    let last_col = (1..=complex::MEDIUM_DENSITY_COLS)
        .rev()
        .find(|col| (last_row + col).is_multiple_of(10))
        .unwrap();
    assert_eq!(
        sheet
            .get(CellRef {
                row: last_row,
                col: last_col
            })
            .unwrap()
            .value,
        Some(CellValue::Number(
            last_row as f64 * 1000.0 + last_col as f64
        ))
    );
}

#[test]
fn dense_merged_grid_resolves_every_block_to_its_own_anchor() {
    // tests/README.md edge-case audit: houganshi_merged only ever tests
    // one merged region, despite Issue #28's original spec calling for
    // merges "至る所に" (everywhere). This tiles BLOCK x BLOCK merges
    // across a SIDE x SIDE grid with no gaps, and verifies every one
    // resolves independently — a cell in one block never leaks into an
    // adjacent block's anchor.
    let workbook = parse_workbook_reader(Cursor::new(complex::dense_merged_grid())).unwrap();
    let sheet = &workbook.sheets()[0];

    let side = complex::DENSE_MERGE_GRID_SIDE;
    let block = complex::DENSE_MERGE_GRID_BLOCK;

    for block_row in 0..side {
        for block_col in 0..side {
            let anchor_row = block_row * block + 1;
            let anchor_col = block_col * block + 1;
            let anchor = sheet
                .get(CellRef {
                    row: anchor_row,
                    col: anchor_col,
                })
                .unwrap();
            let expected_value = (block_row * side + block_col) as f64;
            assert_eq!(
                anchor.value,
                Some(CellValue::Number(expected_value)),
                "block ({block_row}, {block_col}) anchor has the wrong value"
            );

            // Every coordinate in this block resolves to the same anchor,
            // never to a neighboring block's.
            for dr in 0..block {
                for dc in 0..block {
                    let cell_ref = CellRef {
                        row: anchor_row + dr,
                        col: anchor_col + dc,
                    };
                    assert_eq!(
                        sheet.get(cell_ref),
                        Some(anchor),
                        "({block_row},{block_col}) offset ({dr},{dc}) did not resolve to its own block's anchor"
                    );
                }
            }
        }
    }

    // rowSpan/colSpan == BLOCK on every anchor, and only SIDE^2 cells (the
    // anchors) are ever emitted — none of the (SIDE*BLOCK)^2 - SIDE^2
    // virtual coordinates show up as separate JSON cells.
    let json = xlsxparser::to_json_string(&workbook).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let cells = parsed["sheets"][0]["cells"].as_array().unwrap();
    assert_eq!(cells.len(), (side * side) as usize);
    for cell in cells {
        assert_eq!(cell["rowSpan"], block);
        assert_eq!(cell["colSpan"], block);
    }
}
