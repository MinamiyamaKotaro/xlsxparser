// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Integration tests against fixture files sourced from calamine's test
//! corpus (`tests/fixtures/other/`), each reproducing a specific parser gap
//! tracked as a sub-issue of #52.

use xlsxparser::{parse_workbook, CellRef, CellValue, DateTimeValue, Error};

fn fixture_path(name: &str) -> String {
    format!("{}/tests/fixtures/other/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn date_iso_xlsx_resolves_t_d_cells_to_datetime() {
    // Issue #58: `t="d"` (ISO 8601) cells were falling back to
    // `CellValue::Text` instead of being parsed as dates. Covers all three
    // shapes the fixture exercises: date-only, date+time, time-only.
    let workbook = parse_workbook(fixture_path("date_iso.xlsx")).unwrap();
    let sheet = &workbook.sheets()[0];

    assert_eq!(
        sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
        Some(CellValue::DateTime(DateTimeValue {
            year: 2021,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        }))
    );
    assert_eq!(
        sheet.get(CellRef { row: 2, col: 1 }).unwrap().value,
        Some(CellValue::DateTime(DateTimeValue {
            year: 2021,
            month: 1,
            day: 1,
            hour: 10,
            minute: 10,
            second: 10,
        }))
    );
    // Time-only: no date component in the source, so it lands on Excel's
    // own "time of day" convention (serial day 0 = 1899-12-30), matching
    // how resolve/style.rs already decodes a fractional serial < 1.
    assert_eq!(
        sheet.get(CellRef { row: 3, col: 1 }).unwrap().value,
        Some(CellValue::DateTime(DateTimeValue {
            year: 1899,
            month: 12,
            day: 30,
            hour: 10,
            minute: 10,
            second: 10,
        }))
    );
}

#[test]
fn minimal_package_xlsx_resolves_workbook_via_root_rels() {
    // Issue #55: the workbook part path was hardcoded to `xl/workbook.xml`
    // rather than resolved via the package root's `_rels/.rels`, per OPC.
    // This fixture deliberately places `workbook.xml`/`styles.xml`/
    // `sheet1.xml` at the package root (not under `xl/`), discoverable only
    // through `_rels/.rels`'s `officeDocument` relationship.
    //
    // Before the fix this failed at Phase 1 with
    // `MissingRelationshipPart("xl/_rels/workbook.xml.rels")` (the hardcoded
    // path never existed in this package). The fixture's real-world
    // `<c>` elements also happen to omit the `r` attribute throughout
    // (valid per ECMA-376 §18.3.1.4, which marks it optional), which
    // `src/parse/worksheet.rs` does not yet support — tracked separately as
    // Issue #79, unrelated to #55 — so this only asserts that resolution
    // now gets *past* Phase 1 and fails later, in Phase 3, rather than
    // asserting full success.
    let err = parse_workbook(fixture_path("minimal_package.xlsx")).unwrap_err();
    assert!(!matches!(err, Error::MissingRelationshipPart(_)));
    assert!(matches!(
        err,
        Error::MissingRequiredElement { name: "r", .. }
    ));
}
