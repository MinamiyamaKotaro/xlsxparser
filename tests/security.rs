//! Integration tests for Category 5 (セキュリティ) fixtures — proves the
//! `container::sanitize` defenses actually stop each attack end to end.
//! See `tests/fixtures/security.rs` for how each package is generated.

#[path = "fixtures/mod.rs"]
mod fixtures;

use fixtures::security;
use std::io::Cursor;
use xlsxparser::{parse_workbook_reader, parse_workbook_reader_with_limits, Error, SizeLimits};

#[test]
fn zip_bomb_is_capped_before_full_decompression() {
    let tiny_cap = SizeLimits {
        max_entry_size: 1_000_000,
        max_total_size: SizeLimits::default().max_total_size,
    };
    let err =
        parse_workbook_reader_with_limits(Cursor::new(security::zip_bomb()), tiny_cap).unwrap_err();
    assert!(
        matches!(err, Error::ZipBombDetected { .. }),
        "expected Error::ZipBombDetected, got {err:?}"
    );
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
