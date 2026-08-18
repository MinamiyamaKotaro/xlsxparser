//! Integration tests for Category 2 (異常系) fixtures — proves the library
//! returns the matching `Error` variant, and never panics, for each
//! malformed-input scenario. See `tests/fixtures/error.rs` for the
//! packages under test.

#[path = "fixtures/mod.rs"]
mod fixtures;

use fixtures::error;
use std::io::Cursor;
use xlsxparser::{parse_workbook_reader, Error};

#[test]
fn corrupted_xml_is_reported_as_xml_parse_error() {
    let err = parse_workbook_reader(Cursor::new(error::corrupted_xml())).unwrap_err();
    assert!(
        matches!(err, Error::XmlParse { .. }),
        "expected Error::XmlParse, got {err:?}"
    );
}

#[test]
fn missing_relations_is_reported_as_dangling_relationship() {
    let err = parse_workbook_reader(Cursor::new(error::missing_relations())).unwrap_err();
    assert!(
        matches!(err, Error::DanglingRelationship { .. }),
        "expected Error::DanglingRelationship, got {err:?}"
    );
}

#[test]
fn out_of_bounds_sst_index_is_reported_not_panicked() {
    let err = parse_workbook_reader(Cursor::new(error::out_of_bounds_sst())).unwrap_err();
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

#[test]
fn invalid_merge_ref_is_reported_as_invalid_merged_range() {
    let err = parse_workbook_reader(Cursor::new(error::invalid_merge_ref())).unwrap_err();
    assert!(
        matches!(err, Error::InvalidMergedRange { .. }),
        "expected Error::InvalidMergedRange, got {err:?}"
    );
}

#[test]
fn overlapping_col_widths_is_reported_as_invalid_column_width_range() {
    let err = parse_workbook_reader(Cursor::new(error::overlapping_col_widths())).unwrap_err();
    assert!(
        matches!(err, Error::InvalidColumnWidthRange { .. }),
        "expected Error::InvalidColumnWidthRange, got {err:?}"
    );
}

#[test]
fn reversed_col_width_range_is_reported_as_invalid_column_width_range() {
    let err = parse_workbook_reader(Cursor::new(error::reversed_col_width_range())).unwrap_err();
    assert!(
        matches!(err, Error::InvalidColumnWidthRange { .. }),
        "expected Error::InvalidColumnWidthRange, got {err:?}"
    );
}
