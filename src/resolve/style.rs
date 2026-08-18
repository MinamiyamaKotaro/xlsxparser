// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Phase 4: applies resolved cell styles, converting `CellValue::Number` to
//! `CellValue::DateTime` when the applied numFmt represents a date/time.

use crate::error::Error;
use crate::model::{CellValue, DateTimeValue, Sheet, StyleSheet};
use crate::parse::PendingStyle;

/// For each entry in `pending`, looks up `ResolvedStyle` in `stylesheet` and
/// sets it on the corresponding cell in `sheet`. Also converts
/// `CellValue::Number` to `CellValue::DateTime` when an `is_date_time`
/// format is applied. `date1904` selects which of Excel's two date-serial
/// epochs (`<workbookPr date1904="1"/>`, parsed once in Phase 1) the
/// conversion uses — see `serial_to_date_time`.
pub(crate) fn resolve(
    sheet: &mut Sheet,
    pending: &[PendingStyle],
    stylesheet: &StyleSheet,
    date1904: bool,
) -> Result<(), Error> {
    for entry in pending {
        let resolved = stylesheet
            .get(&entry.style_id)
            .ok_or(Error::InvalidStyleId(entry.style_id))?
            .clone();
        // Assumes Phase 3 has already inserted a cell at the same cell_ref
        // (parse/worksheet.rs's calling contract).
        let cell = sheet
            .get_mut(entry.cell_ref)
            .expect("pending style references a cell not inserted by parse/worksheet.rs");
        if resolved.is_date_time {
            if let Some(CellValue::Number(serial)) = cell.value {
                // Leave CellValue::Number unchanged when conversion isn't
                // possible (fallback — see docs/design/resolve/style.en.md
                // Error Handling Policy).
                if let Some(dt) = serial_to_date_time(serial, date1904) {
                    cell.value = Some(CellValue::DateTime(dt));
                }
            }
        }
        cell.style = Some(resolved);
    }
    Ok(())
}

/// Days (Unix epoch, 1970-01-01) of the day *before* each of Excel's own
/// date-serial epochs, i.e. the offset such that `serial - offset` is a
/// correct real-calendar day count for `serial >= 61` in the 1900 system,
/// and for every `serial` in the 1904 system.
///
/// 1900 system: equivalent to epoch 1899-12-30. Only correct as-is from
/// serial 61 (1900-03-01) onward — see [`excel_serial_day_to_ymd`] for the
/// `serial` 1-60 special-casing this offset alone does *not* cover.
///
/// 1904 system: equivalent to epoch 1904-01-01 (no leap-year bug — 1904 is
/// a genuine leap year, so no special-casing is needed here). Verified
/// directly: 66 years span 1904-01-01..1970-01-01, containing 17 leap
/// years (1904, 1908, ..., 1968) and 49 common years, i.e.
/// 17*366 + 49*365 = 24,107 days.
const EPOCH_OFFSET_1900: i64 = 25_569;
const EPOCH_OFFSET_1904: i64 = 24_107;

/// Converts an Excel serial value into a `DateTimeValue`, honoring
/// `date1904`. Returns `None` (not an error) for values that cannot be
/// converted: negative, `NaN`, `Infinity`, or beyond Excel's representable
/// range (up to 9999-12-31, serial 2,958,465).
fn serial_to_date_time(serial: f64, date1904: bool) -> Option<DateTimeValue> {
    const EXCEL_MAX_SERIAL: f64 = 2_958_465.0; // 9999-12-31

    if !serial.is_finite() || !(0.0..=EXCEL_MAX_SERIAL).contains(&serial) {
        return None;
    }

    let mut serial_day = serial.trunc() as i64;
    // serial >= 0.0 is already guaranteed above, so fract() is in [0.0, 1.0)
    // and this can never go negative.
    let mut total_seconds = (serial.fract() * 86_400.0).round() as i64;
    if total_seconds >= 86_400 {
        // Rounding a fractional part extremely close to 1.0 (e.g.
        // 0.999999994) up to a full 86,400 seconds would otherwise silently
        // roll into the *next* day's midnight while serial_day still points
        // at the current day — carry the day forward explicitly instead.
        total_seconds -= 86_400;
        serial_day += 1;
    }

    let (year, month, day) = excel_serial_day_to_ymd(serial_day, date1904);
    let hour = (total_seconds / 3_600) as u8;
    let minute = ((total_seconds / 60) % 60) as u8;
    let second = (total_seconds % 60) as u8;

    Some(DateTimeValue {
        year,
        month,
        day,
        hour,
        minute,
        second,
    })
}

/// Converts the integer-day part of an Excel serial value into (year,
/// month, day), honoring `date1904`.
///
/// Excel's 1900 leap-year bug (Microsoft KB214326) means `EPOCH_OFFSET_1900`
/// alone is *not* sufficient: verified directly (see the design discussion
/// on Issue #40) that plain proleptic-Gregorian arithmetic from that offset
/// lands one day early for every `serial` in `1..60` (e.g. `serial` 1 comes
/// out as 1899-12-31, not Excel's own 1900-01-01) and cannot represent
/// `serial` 60 at all, since the real Gregorian calendar has no
/// "1900-02-29" (1900 is not actually a leap year) for it to land on. The
/// fix used here is the same one openpyxl and most other Excel-compatible
/// readers use: shift `serial` values 1-59 forward by one day before
/// applying the offset (this also incidentally makes 61 onward already
/// correct unshifted, since that phantom day has been "absorbed"), and
/// hardcode serial 60 directly to the fictitious date Excel itself reports
/// for it, since no calendar arithmetic can derive it.
fn excel_serial_day_to_ymd(serial_day: i64, date1904: bool) -> (i32, u8, u8) {
    if !date1904 && serial_day == 60 {
        return (1900, 2, 29);
    }
    let shift = i64::from(!date1904 && (1..60).contains(&serial_day));
    let offset = if date1904 {
        EPOCH_OFFSET_1904
    } else {
        EPOCH_OFFSET_1900
    };
    civil_from_days(serial_day + shift - offset)
}

/// Converts a day count since 1970-01-01 (proleptic Gregorian, may be
/// negative) into (year, month, day). Howard Hinnant's `civil_from_days`
/// algorithm — public domain,
/// <http://howardhinnant.github.io/date_algorithms.html> — chosen so this
/// crate can support the full serial-date range without depending on an
/// external date/time crate (Issue #40's stated performance requirement:
/// avoid `chrono`'s compile-time/binary-size cost when a self-contained
/// integer algorithm suffices).
fn civil_from_days(z: i64) -> (i32, u8, u8) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u8, d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Cell, CellRef, ResolvedStyle, SheetVisibility};
    use std::sync::Arc;

    fn sheet_with_cell(cell_ref: CellRef, value: Option<CellValue>) -> Sheet {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(cell_ref, Cell { value, style: None });
        sheet
    }

    #[test]
    fn applies_resolved_style_to_cell() {
        let cell_ref = CellRef { row: 1, col: 1 };
        let mut sheet = sheet_with_cell(cell_ref, Some(CellValue::Number(1.0)));
        let mut stylesheet = StyleSheet::new();
        stylesheet.insert(
            0,
            Arc::new(ResolvedStyle {
                is_date_time: false,
                ..Default::default()
            }),
        );

        resolve(
            &mut sheet,
            &[PendingStyle {
                cell_ref,
                style_id: 0,
            }],
            &stylesheet,
            false,
        )
        .unwrap();

        assert!(sheet.get(cell_ref).unwrap().style.is_some());
        assert!(
            !sheet
                .get(cell_ref)
                .unwrap()
                .style
                .as_ref()
                .unwrap()
                .is_date_time
        );
    }

    #[test]
    fn nonexistent_style_id_is_an_error() {
        let cell_ref = CellRef { row: 1, col: 1 };
        let mut sheet = sheet_with_cell(cell_ref, Some(CellValue::Number(1.0)));
        let stylesheet = StyleSheet::new();

        let err = resolve(
            &mut sheet,
            &[PendingStyle {
                cell_ref,
                style_id: 99,
            }],
            &stylesheet,
            false,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidStyleId(99)));
    }

    #[test]
    fn date_time_style_converts_convertible_number() {
        let cell_ref = CellRef { row: 1, col: 1 };
        let mut sheet = sheet_with_cell(cell_ref, Some(CellValue::Number(45000.0)));
        let mut stylesheet = StyleSheet::new();
        stylesheet.insert(
            0,
            Arc::new(ResolvedStyle {
                is_date_time: true,
                ..Default::default()
            }),
        );

        resolve(
            &mut sheet,
            &[PendingStyle {
                cell_ref,
                style_id: 0,
            }],
            &stylesheet,
            false,
        )
        .unwrap();

        // Serial 45000 (1900 system) is 2023-03-15 — a well-known reference
        // point for sanity-checking Excel serial-date conversions.
        assert_eq!(
            sheet.get(cell_ref).unwrap().value,
            Some(CellValue::DateTime(DateTimeValue {
                year: 2023,
                month: 3,
                day: 15,
                hour: 0,
                minute: 0,
                second: 0,
            }))
        );
    }

    #[test]
    fn date_time_style_leaves_unconvertible_number_unchanged() {
        let cell_ref = CellRef { row: 1, col: 1 };
        let mut sheet = sheet_with_cell(cell_ref, Some(CellValue::Number(-5.0)));
        let mut stylesheet = StyleSheet::new();
        stylesheet.insert(
            0,
            Arc::new(ResolvedStyle {
                is_date_time: true,
                ..Default::default()
            }),
        );

        resolve(
            &mut sheet,
            &[PendingStyle {
                cell_ref,
                style_id: 0,
            }],
            &stylesheet,
            false,
        )
        .unwrap();

        assert_eq!(
            sheet.get(cell_ref).unwrap().value,
            Some(CellValue::Number(-5.0))
        );
    }

    #[test]
    fn date_time_style_on_non_numeric_value_is_left_unconverted() {
        let cell_ref = CellRef { row: 1, col: 1 };
        let mut sheet = sheet_with_cell(cell_ref, Some(CellValue::Boolean(true)));
        let mut stylesheet = StyleSheet::new();
        stylesheet.insert(
            0,
            Arc::new(ResolvedStyle {
                is_date_time: true,
                ..Default::default()
            }),
        );

        resolve(
            &mut sheet,
            &[PendingStyle {
                cell_ref,
                style_id: 0,
            }],
            &stylesheet,
            false,
        )
        .unwrap();

        assert_eq!(
            sheet.get(cell_ref).unwrap().value,
            Some(CellValue::Boolean(true))
        );
    }

    #[test]
    fn non_date_time_style_only_sets_style() {
        let cell_ref = CellRef { row: 1, col: 1 };
        let mut sheet = sheet_with_cell(cell_ref, Some(CellValue::Number(3.0)));
        let mut stylesheet = StyleSheet::new();
        stylesheet.insert(
            0,
            Arc::new(ResolvedStyle {
                is_date_time: false,
                ..Default::default()
            }),
        );

        resolve(
            &mut sheet,
            &[PendingStyle {
                cell_ref,
                style_id: 0,
            }],
            &stylesheet,
            false,
        )
        .unwrap();

        assert_eq!(
            sheet.get(cell_ref).unwrap().value,
            Some(CellValue::Number(3.0))
        );
    }

    #[test]
    fn shared_style_arcs_are_not_duplicated() {
        let ref_a = CellRef { row: 1, col: 1 };
        let ref_b = CellRef { row: 1, col: 2 };
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            ref_a,
            Cell {
                value: None,
                style: None,
            },
        );
        sheet.insert_cell(
            ref_b,
            Cell {
                value: None,
                style: None,
            },
        );
        let mut stylesheet = StyleSheet::new();
        stylesheet.insert(
            0,
            Arc::new(ResolvedStyle {
                is_date_time: false,
                ..Default::default()
            }),
        );

        resolve(
            &mut sheet,
            &[
                PendingStyle {
                    cell_ref: ref_a,
                    style_id: 0,
                },
                PendingStyle {
                    cell_ref: ref_b,
                    style_id: 0,
                },
            ],
            &stylesheet,
            false,
        )
        .unwrap();

        assert!(Arc::ptr_eq(
            sheet.get(ref_a).unwrap().style.as_ref().unwrap(),
            sheet.get(ref_b).unwrap().style.as_ref().unwrap()
        ));
    }

    #[test]
    fn empty_pending_list_is_a_no_op() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let stylesheet = StyleSheet::new();
        resolve(&mut sheet, &[], &stylesheet, false).unwrap();
    }

    #[test]
    fn serial_to_date_time_rejects_non_finite_and_negative() {
        assert_eq!(serial_to_date_time(f64::NAN, false), None);
        assert_eq!(serial_to_date_time(f64::INFINITY, false), None);
        assert_eq!(serial_to_date_time(-1.0, false), None);
        assert_eq!(serial_to_date_time(3_000_000.0, false), None);
    }

    #[test]
    fn serial_1_is_1900_01_01_not_the_naive_epoch_minus_one() {
        // The naive "epoch = 1899-12-30, no correction" arithmetic that
        // EPOCH_OFFSET_1900 alone implies would put serial 1 at
        // 1899-12-31 — one day off from what Excel itself reports
        // (1900-01-01). Regression test for the `excel_serial_day_to_ymd`
        // shift.
        assert_eq!(
            serial_to_date_time(1.0, false),
            Some(DateTimeValue {
                year: 1900,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            })
        );
    }

    #[test]
    fn serial_59_and_61_bracket_the_1900_leap_year_bug_correctly() {
        assert_eq!(
            serial_to_date_time(59.0, false).map(|d| (d.year, d.month, d.day)),
            Some((1900, 2, 28))
        );
        assert_eq!(
            serial_to_date_time(61.0, false).map(|d| (d.year, d.month, d.day)),
            Some((1900, 3, 1))
        );
    }

    #[test]
    fn serial_60_is_the_fictitious_1900_02_29_leap_year_bug() {
        // Microsoft KB214326: Excel documents serial 60 as representing
        // "February 29, 1900" even though 1900 is not actually a leap
        // year — this date has no real proleptic-Gregorian equivalent and
        // is hardcoded rather than derived.
        assert_eq!(
            serial_to_date_time(60.0, false).map(|d| (d.year, d.month, d.day)),
            Some((1900, 2, 29))
        );
    }

    #[test]
    fn date1904_system_has_no_leap_year_bug() {
        // 1904 is a genuine leap year, so unlike the 1900 system, serial 60
        // in the 1904 system is an ordinary, non-fictitious date.
        assert_eq!(
            serial_to_date_time(0.0, true).map(|d| (d.year, d.month, d.day)),
            Some((1904, 1, 1))
        );
        assert_eq!(
            serial_to_date_time(1.0, true).map(|d| (d.year, d.month, d.day)),
            Some((1904, 1, 2))
        );
        assert_eq!(
            serial_to_date_time(60.0, true).map(|d| (d.year, d.month, d.day)),
            Some((1904, 3, 1))
        );
    }

    #[test]
    fn fractional_serial_decomposes_into_time_of_day() {
        // 45000.5 -> 2023-03-15, noon.
        assert_eq!(
            serial_to_date_time(45000.5, false),
            Some(DateTimeValue {
                year: 2023,
                month: 3,
                day: 15,
                hour: 12,
                minute: 0,
                second: 0,
            })
        );
    }

    #[test]
    fn rounding_a_fractional_part_up_to_a_full_day_carries_into_the_next_day() {
        // 0.9999999999 * 86400 rounds to exactly 86400 seconds — must carry
        // into day 3 at 00:00:00 rather than reporting day 2 at 24:00:00 (an
        // otherwise-impossible `hour: 24`).
        let result = serial_to_date_time(2.999_999_999_9, false).unwrap();
        assert_eq!((result.year, result.month, result.day), (1900, 1, 3));
        assert_eq!((result.hour, result.minute, result.second), (0, 0, 0));
    }
}
