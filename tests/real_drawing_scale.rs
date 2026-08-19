//! Integration test for Issue #71's follow-up question ("does parsing many
//! images stay O(N), not O(N^2), in image count?") against a real,
//! openpyxl-adjacent `.xlsx` file — see
//! `scripts/generate_real_fixtures.py`'s `many_images()`. Complements the
//! ad hoc, non-committed scaling benchmarks run during that issue's
//! discussion by keeping a permanent correctness check (not a timing
//! assertion, which would be flaky across CI hardware) that every one of a
//! large number of images resolves correctly.

use xlsxparser::{parse_workbook, CellRef, ImageAnchor};

fn fixture_path(relative: &str) -> String {
    format!("{}/tests/fixtures/{relative}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn real_many_images_xlsx_resolves_every_one_of_two_hundred_images() {
    let workbook = parse_workbook(fixture_path("load/many_images.xlsx")).unwrap();
    let sheet = &workbook.sheets()[0];

    let images = sheet.images();
    assert_eq!(images.len(), 200);

    // scripts/generate_real_fixtures.py's many_images() anchors image `i`
    // (0-based) at row `i`..`i+1`, column A..B — 0-based xdr:row=i becomes
    // 1-based CellRef.row = i + 1, and each image's target is
    // xl/media/image{i}.png via its own rIdEmbed{i}.
    let first = &images[0];
    assert_eq!(first.target, "xl/media/image0.png");
    match first.anchor {
        ImageAnchor::TwoCell { from, to } => {
            assert_eq!(from.cell, CellRef { row: 1, col: 1 });
            assert_eq!(to.cell, CellRef { row: 2, col: 2 });
        }
        ImageAnchor::OneCell { .. } => panic!("expected a TwoCell anchor"),
    }

    let last = &images[199];
    assert_eq!(last.target, "xl/media/image199.png");
    match last.anchor {
        ImageAnchor::TwoCell { from, to } => {
            assert_eq!(from.cell, CellRef { row: 200, col: 1 });
            assert_eq!(to.cell, CellRef { row: 201, col: 2 });
        }
        ImageAnchor::OneCell { .. } => panic!("expected a TwoCell anchor"),
    }

    // Every image in between resolves too, in document order, with no
    // duplicates or gaps introduced by the parse — the actual property the
    // O(N)-not-O(N^2) scaling question was about.
    for (i, image) in images.iter().enumerate() {
        assert_eq!(image.target, format!("xl/media/image{i}.png"));
    }
}
