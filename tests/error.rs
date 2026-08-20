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
fn corrupted_workbook_xml_is_reported_as_xml_parse_error() {
    // tests/README.md edge-case audit: only worksheet.xml had a dedicated
    // malformed-XML fixture prior to this one.
    let err = parse_workbook_reader(Cursor::new(error::corrupted_workbook_xml())).unwrap_err();
    assert!(
        matches!(err, Error::XmlParse { .. }),
        "expected Error::XmlParse, got {err:?}"
    );
}

#[test]
fn corrupted_styles_xml_is_reported_as_xml_parse_error() {
    let err = parse_workbook_reader(Cursor::new(error::corrupted_styles_xml())).unwrap_err();
    assert!(
        matches!(err, Error::XmlParse { .. }),
        "expected Error::XmlParse, got {err:?}"
    );
}

#[test]
fn corrupted_shared_strings_xml_is_reported_as_missing_closing_tag() {
    // Not Error::XmlParse: this fixture's truncation point (EOF inside a
    // well-formed <t> open tag, no closing </si>) is caught by
    // concat_rich_text's own explicit EOF check rather than a raw XML
    // syntax error — see the fixture's doc comment.
    let err =
        parse_workbook_reader(Cursor::new(error::corrupted_shared_strings_xml())).unwrap_err();
    assert!(
        matches!(
            err,
            Error::MissingRequiredElement {
                name: "si/is closing tag",
                ..
            }
        ),
        "expected Error::MissingRequiredElement{{name: \"si/is closing tag\"}}, got {err:?}"
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
