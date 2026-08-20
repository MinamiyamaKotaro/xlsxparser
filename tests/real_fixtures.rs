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
fn real_basic_types_xlsx_carries_an_unused_theme_part_that_is_never_read() {
    // Issue #76 "pay-for-what-you-use", confirmed against a genuine
    // third-party writer's output rather than a synthetic canary: openpyxl
    // always emits xl/theme/theme1.xml (its default Office theme) even for
    // a workbook whose cells never reference a theme color, as this one's
    // do not. Workbook::theme() must come back None — the part existing on
    // disk is not enough to trigger parsing it.
    let workbook = parse_workbook(fixture_path("normal/basic_types.xlsx")).unwrap();
    assert!(workbook.theme().is_none());
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
fn real_cell_hyperlinks_xlsx_resolves_external_and_location_only_hyperlinks() {
    // scripts/generate_real_fixtures.py's cell_hyperlinks(): a real,
    // openpyxl-authored external hyperlink (assigned relationship id,
    // TargetMode="External", tooltip) plus a location-only internal jump
    // with no r:id at all, resolved end to end through actual ZIP/XML
    // bytes rather than the hand-authored snippets pipeline.rs's own unit
    // tests use — the standard rationale this whole file exists for (see
    // its module doc). Notably, openpyxl happens to declare xmlns:r
    // inline on the <hyperlink> element itself, rather than anywhere
    // hand-authored fixtures in this repo ever place it (they never
    // declare it at all) — parse/worksheet.rs's plain string-prefix
    // attribute matching (parse/mod.md Open Question 4's policy) is
    // indifferent to either shape by design, and this pins that down
    // against a real writer's actual output instead of leaving it an
    // untested assumption.
    let workbook = parse_workbook(fixture_path("complex/cell_hyperlinks.xlsx")).unwrap();
    let sheet = &workbook.sheets()[0];

    let external = sheet
        .hyperlink_at(CellRef { row: 2, col: 1 })
        .expect("A2 must carry a hyperlink");
    assert_eq!(external.target.as_deref(), Some("https://example.com/"));
    assert_eq!(external.location, None);
    assert_eq!(external.tooltip.as_deref(), Some("Visit example"));

    let internal = sheet
        .hyperlink_at(CellRef { row: 3, col: 1 })
        .expect("A3 must carry a hyperlink");
    assert_eq!(internal.target, None);
    assert_eq!(internal.location.as_deref(), Some("Sheet1!A1"));
    assert_eq!(internal.tooltip, None);
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
fn real_embedded_image_one_cell_xlsx_resolves_a_single_cell_confined_anchor() {
    use xlsxparser::{AnchorMarker, ImageAnchor, ImageExtent};

    let workbook = parse_workbook(fixture_path("complex/embedded_image_one_cell.xlsx")).unwrap();
    let sheet = &workbook.sheets()[0];

    let images = sheet.images();
    assert_eq!(images.len(), 1);
    let image = &images[0];

    assert_eq!(image.target, "xl/media/image1.png");
    // No <a:hlinkClick> on this fixture's <xdr:pic>.
    assert_eq!(image.hyperlink, None);
    // scripts/generate_real_fixtures.py's embedded_image_one_cell() anchors
    // at 0-based xdr:col=2/xdr:row=4 -> 1-based CellRef { row: 5, col: 3 }
    // (C5), sized well under a default cell's EMU dimensions.
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
    assert_eq!(json_image["anchor"]["ext"]["cx"], 400_000);
    // hyperlink is omitted entirely (not null) when the image has none.
    assert!(json_image.get("hyperlink").is_none());
}

#[test]
fn real_grouped_images_xlsx_resolves_relative_to_group_with_per_pic_hyperlink_scoping() {
    use xlsxparser::{AnchorMarker, ImageAnchor, ImageExtent};

    let workbook = parse_workbook(fixture_path("complex/grouped_images.xlsx")).unwrap();
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
    // scripts/generate_real_fixtures.py's _grouped_images_anchor_xml()
    // places the second pic 1_080_000 EMU right of the first, added to the
    // anchor's own colOff.
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

#[test]
fn real_styled_fill_color_xlsx_resolves_rgb_and_theme_fills() {
    use xlsxparser::ColorRef;

    let workbook = parse_workbook(fixture_path("complex/styled_fill_color.xlsx")).unwrap();
    let sheet = &workbook.sheets()[0];

    // scripts/generate_real_fixtures.py's styled_fill_color(): A1 an RGB
    // solid fill, A2 a theme+tint solid fill, A3 no fill at all.
    let a1 = sheet.get(CellRef { row: 1, col: 1 }).unwrap();
    assert_eq!(
        a1.style.as_ref().unwrap().fill_fg_color,
        Some(ColorRef::Rgb(Arc::from("FFFF0000")))
    );

    let a2 = sheet.get(CellRef { row: 2, col: 1 }).unwrap();
    assert_eq!(
        a2.style.as_ref().unwrap().fill_fg_color,
        Some(ColorRef::Theme {
            index: 4,
            tint: Some(-0.25)
        })
    );

    let a3 = sheet.get(CellRef { row: 3, col: 1 }).unwrap();
    // A3 carries no explicit style at all (openpyxl omits the `s` attribute
    // for the default style), so it has no fill color to report either.
    let a3_fill_fg = a3.style.as_ref().and_then(|s| s.fill_fg_color.clone());
    assert_eq!(a3_fill_fg, None);
}

#[test]
fn real_styled_fill_color_xlsx_resolves_theme_palette_and_display_rgb() {
    // Issue #76, against the same real openpyxl-written file as the test
    // above (rather than the hand-authored XML every parse::theme/
    // resolve::color unit test uses) — this is the one test in the whole
    // suite that proves the feature works end to end against a genuine
    // third-party writer's output, not just this crate's own understanding
    // of the format encoded into synthetic fixtures.
    use xlsxparser::{resolve_color, Rgb};

    let workbook = parse_workbook(fixture_path("complex/styled_fill_color.xlsx")).unwrap();

    // openpyxl's default Office theme: accent1 (theme index 4) is
    // #4F81BD, at ThemePalette's slot-0/1-swapped index contract (Issue
    // #76 PoC, confirmed against this exact fixture's xl/theme/theme1.xml).
    let theme = workbook
        .theme()
        .expect("styled_fill_color.xlsx carries a theme relationship (rId3 -> theme/theme1.xml)");
    assert_eq!(
        theme.0[4],
        Rgb {
            r: 0x4F,
            g: 0x81,
            b: 0xBD
        }
    );

    // A2's theme=4/tint=-0.25 fgColor resolves to the real displayed color
    // through the public resolve_color API, matching the PoC-verified
    // value from Issue #76's design comments.
    let sheet = &workbook.sheets()[0];
    let a2_color = sheet
        .get(CellRef { row: 2, col: 1 })
        .unwrap()
        .style
        .as_ref()
        .unwrap()
        .fill_fg_color
        .as_ref()
        .unwrap();
    assert_eq!(
        resolve_color(a2_color, workbook.theme()),
        Some(Rgb {
            r: 0x37,
            g: 0x60,
            b: 0x92
        })
    );

    // A1's plain RGB fill resolves too, independent of the theme palette.
    let a1_color = sheet
        .get(CellRef { row: 1, col: 1 })
        .unwrap()
        .style
        .as_ref()
        .unwrap()
        .fill_fg_color
        .as_ref()
        .unwrap();
    assert_eq!(
        resolve_color(a1_color, workbook.theme()),
        Some(Rgb {
            r: 0xFF,
            g: 0x00,
            b: 0x00
        })
    );
}
