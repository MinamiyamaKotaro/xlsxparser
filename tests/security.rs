//! Integration tests for Category 5 (セキュリティ) fixtures — proves the
//! `container::sanitize` defenses actually stop each attack end to end.
//! See `tests/fixtures/security.rs` for how each package is generated.

#[path = "fixtures/mod.rs"]
mod fixtures;

use fixtures::security;
use std::io::Cursor;
use xlsxparser::{
    parse_workbook_reader, parse_workbook_reader_with_limits, to_json_string, Error, SizeLimits,
};

#[test]
fn sparse_merge_bounding_box_does_not_amplify_json_generation_cost() {
    // Issue #43: a legitimate file (every individual limit — merge count,
    // cell count, byte size — respected) that arranges 20,000 merges to
    // maximize `merge_bounds` used to make `json.rs`'s `iter_cells` fall
    // back to an O(merged regions) scan for each of 300,000 unrelated
    // cells. Doesn't assert wall-clock time (CI hardware varies too much,
    // per `tests/fixtures/load.rs`'s stated convention) — completing at
    // all within the test run, rather than stalling for the tens of
    // seconds directly measured pre-fix, is the signal.
    let workbook = parse_workbook_reader(Cursor::new(
        security::sparse_merge_bounding_box_amplification(),
    ))
    .unwrap();

    // Pins down the fixture's actual cell count: 300,000 distinct data
    // cells plus 20,000 merge origins (each of the 19,999 filler merges
    // backfills its own blank origin cell, plus the far-corner one). Catches
    // the fixture silently collapsing distinct cells into fewer unique
    // `CellRef`s (a coordinate-collision bug once shipped here — a fixed
    // row for every data cell wrapped every 16,384 columns, so only 16,384
    // of the intended 300,000 cells were ever actually distinct).
    assert_eq!(workbook.sheets()[0].iter_cells().count(), 320_000);

    let json = to_json_string(&workbook).unwrap();
    assert!(json.contains("\"maxRow\":1048576"));
}

#[test]
fn zip_bomb_is_capped_before_full_decompression() {
    let tiny_cap = SizeLimits {
        max_entry_size: 1_000_000,
        ..SizeLimits::default()
    };
    let err =
        parse_workbook_reader_with_limits(Cursor::new(security::zip_bomb()), tiny_cap).unwrap_err();
    assert!(
        matches!(err, Error::ZipBombDetected { .. }),
        "expected Error::ZipBombDetected, got {err:?}"
    );
}

#[test]
fn too_many_cells_is_rejected_before_exceeding_the_cap() {
    let tiny_cap = SizeLimits {
        max_cells_per_sheet: 3,
        ..SizeLimits::default()
    };
    let err = parse_workbook_reader_with_limits(Cursor::new(security::too_many_cells(4)), tiny_cap)
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::TooManyCells {
                count: 4,
                limit: 3,
                ..
            }
        ),
        "expected Error::TooManyCells {{ count: 4, limit: 3, .. }}, got {err:?}"
    );

    // One cell under the cap still parses successfully.
    parse_workbook_reader_with_limits(Cursor::new(security::too_many_cells(3)), tiny_cap).unwrap();
}

#[test]
fn zip_slip_entry_name_rejects_the_whole_archive() {
    let err = parse_workbook_reader(Cursor::new(security::zip_slip())).unwrap_err();
    assert!(
        matches!(err, Error::ZipSlipDetected { .. }),
        "expected Error::ZipSlipDetected, got {err:?}"
    );
}

#[test]
fn xxe_doctype_is_rejected_without_entity_expansion() {
    let err = parse_workbook_reader(Cursor::new(security::xxe_attack())).unwrap_err();
    assert!(
        matches!(err, Error::DoctypeRejected { .. }),
        "expected Error::DoctypeRejected, got {err:?}"
    );
}
