//! Phase 4: applies resolved cell styles, converting `CellValue::Number` to
//! `CellValue::DateTime` when the applied numFmt represents a date/time.

use crate::error::Error;
use crate::model::{CellValue, DateTimeValue, Sheet, StyleSheet};
use crate::parse::PendingStyle;

/// For each entry in `pending`, looks up `ResolvedStyle` in `stylesheet` and
/// sets it on the corresponding cell in `sheet`. Also converts
/// `CellValue::Number` to `CellValue::DateTime` when an `is_date_time`
/// format is applied.
pub(crate) fn resolve(
    sheet: &mut Sheet,
    pending: &[PendingStyle],
    stylesheet: &StyleSheet,
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
                if let Some(dt) = serial_to_date_time(serial) {
                    cell.value = Some(CellValue::DateTime(dt));
                }
            }
        }
        cell.style = Some(resolved);
    }
    Ok(())
}

/// Converts an Excel serial value into a `DateTimeValue`. Returns `None`
/// (not an error) for values that cannot be converted: negative, `NaN`,
/// `Infinity`, or beyond Excel's representable range (up to 9999-12-31,
/// serial 2,958,465). `DateTimeValue` is currently a data-less placeholder
/// (see docs/design/model/cell.en.md Open Question 4), so this only
/// validates plausibility; it does not yet retain the calendar
/// decomposition (year/month/day/...), including the 1900 leap-year bug.
fn serial_to_date_time(serial: f64) -> Option<DateTimeValue> {
    const EXCEL_MAX_SERIAL: f64 = 2_958_465.0; // 9999-12-31

    if !serial.is_finite() || !(0.0..=EXCEL_MAX_SERIAL).contains(&serial) {
        return None;
    }
    Some(DateTimeValue)
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
            }),
        );

        resolve(
            &mut sheet,
            &[PendingStyle {
                cell_ref,
                style_id: 0,
            }],
            &stylesheet,
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
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidStyleId(99)));
    }

    #[test]
    fn date_time_style_converts_convertible_number() {
        let cell_ref = CellRef { row: 1, col: 1 };
        let mut sheet = sheet_with_cell(cell_ref, Some(CellValue::Number(45000.0)));
        let mut stylesheet = StyleSheet::new();
        stylesheet.insert(0, Arc::new(ResolvedStyle { is_date_time: true }));

        resolve(
            &mut sheet,
            &[PendingStyle {
                cell_ref,
                style_id: 0,
            }],
            &stylesheet,
        )
        .unwrap();

        assert_eq!(
            sheet.get(cell_ref).unwrap().value,
            Some(CellValue::DateTime(DateTimeValue))
        );
    }

    #[test]
    fn date_time_style_leaves_unconvertible_number_unchanged() {
        let cell_ref = CellRef { row: 1, col: 1 };
        let mut sheet = sheet_with_cell(cell_ref, Some(CellValue::Number(-5.0)));
        let mut stylesheet = StyleSheet::new();
        stylesheet.insert(0, Arc::new(ResolvedStyle { is_date_time: true }));

        resolve(
            &mut sheet,
            &[PendingStyle {
                cell_ref,
                style_id: 0,
            }],
            &stylesheet,
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
        stylesheet.insert(0, Arc::new(ResolvedStyle { is_date_time: true }));

        resolve(
            &mut sheet,
            &[PendingStyle {
                cell_ref,
                style_id: 0,
            }],
            &stylesheet,
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
            }),
        );

        resolve(
            &mut sheet,
            &[PendingStyle {
                cell_ref,
                style_id: 0,
            }],
            &stylesheet,
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
        resolve(&mut sheet, &[], &stylesheet).unwrap();
    }

    #[test]
    fn serial_to_date_time_rejects_non_finite_and_negative() {
        assert_eq!(serial_to_date_time(f64::NAN), None);
        assert_eq!(serial_to_date_time(f64::INFINITY), None);
        assert_eq!(serial_to_date_time(-1.0), None);
        assert_eq!(serial_to_date_time(3_000_000.0), None);
        assert_eq!(serial_to_date_time(0.0), Some(DateTimeValue));
        assert_eq!(serial_to_date_time(45000.0), Some(DateTimeValue));
    }
}
