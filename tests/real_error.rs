//! Integration tests for Category 2 (異常系) against real, openpyxl-authored
//! `.xlsx` files that were then mutated the way a real-world failure would
//! corrupt them (truncated mid-write, a renumbered relationship ID, a
//! reversed merge ref) — see `scripts/generate_real_fixtures.py`'s
//! `corrupted_xml`/`missing_relations`/`invalid_merge_ref`/
//! `out_of_bounds_sst` for exactly what was mutated and why. Complements
//! `tests/error.rs`, whose fixtures are invalid XML from the very first
//! byte rather than a corrupted genuine file.

use xlsxparser::{parse_workbook, Error};

fn fixture_path(relative: &str) -> String {
    format!("{}/tests/fixtures/{relative}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn real_corrupted_xml_is_reported_as_xml_parse_error() {
    let err = parse_workbook(fixture_path("error/corrupted_xml.xlsx")).unwrap_err();
    assert!(
        matches!(err, Error::XmlParse { .. }),
        "expected Error::XmlParse, got {err:?}"
    );
}

#[test]
fn real_missing_relations_is_reported_as_dangling_relationship() {
    let err = parse_workbook(fixture_path("error/missing_relations.xlsx")).unwrap_err();
    assert!(
        matches!(err, Error::DanglingRelationship { .. }),
        "expected Error::DanglingRelationship, got {err:?}"
    );
}

#[test]
fn real_invalid_merge_ref_is_reported_as_invalid_merged_range() {
    let err = parse_workbook(fixture_path("error/invalid_merge_ref.xlsx")).unwrap_err();
    assert!(
        matches!(err, Error::InvalidMergedRange { .. }),
        "expected Error::InvalidMergedRange, got {err:?}"
    );
}

#[test]
fn real_out_of_bounds_sst_index_is_reported_not_panicked() {
    let err = parse_workbook(fixture_path("error/out_of_bounds_sst.xlsx")).unwrap_err();
    assert!(
        matches!(
            err,
            Error::SharedStringIndexOutOfBounds {
                index: 99999,
                len: 1
            }
        ),
        "expected Error::SharedStringIndexOutOfBounds{{index: 99999, len: 1}}, got {err:?}"
    );
}
