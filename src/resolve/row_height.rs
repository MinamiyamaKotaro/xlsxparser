// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Phase 4: validates the row-height range list `parse/worksheet.rs`
//! already compressed while streaming `<sheetData>`, before registering it
//! with `Sheet::set_row_heights` — the row-axis counterpart of
//! `resolve::column_width` (Issue #51).

use crate::error::Error;
use crate::model::{RowHeightRange, Sheet};

/// Cap on the number of row-height range entries accepted for a single
/// sheet. Unlike `resolve::column_width::MAX_COLUMN_WIDTH_RANGES`, this
/// doesn't bound an XML-declared range count directly — `<row>` is never
/// itself a range — but a pathological file alternating row heights every
/// row defeats `parse/worksheet.rs`'s compression entirely (one range per
/// row), so `R` still needs its own bound independent of byte size, same
/// reasoning as `MAX_COLUMN_WIDTH_RANGES`.
pub(crate) const MAX_ROW_HEIGHT_RANGES: usize = 2_000;

/// Validates `ranges` and registers them into `sheet`, along with
/// `default_row_height` (from `<sheetFormatPr defaultRowHeight="..">`, if
/// present).
///
/// Rejects a batch larger than [`MAX_ROW_HEIGHT_RANGES`] as
/// `Error::TooManyRowHeightRanges` before doing any sorting. Unlike
/// `column_width::resolve`, there is no reversed-`min`/`max` check here:
/// `parse/worksheet.rs`'s `push_row_height` only ever builds a range with
/// `min <= max` by construction. Ranges are still sorted by `min` — O(R log
/// R) — and checked for overlap between adjacent pairs
/// (`Error::InvalidRowHeightRange`), fail-closed: this only matters for a
/// file whose `<row>` elements aren't in strictly ascending `r` order (the
/// common case is already sorted, since it was built by a single forward
/// pass over document order).
pub(crate) fn resolve(
    sheet: &mut Sheet,
    mut ranges: Vec<RowHeightRange>,
    default_row_height: Option<f64>,
) -> Result<(), Error> {
    if ranges.len() > MAX_ROW_HEIGHT_RANGES {
        return Err(Error::TooManyRowHeightRanges {
            count: ranges.len(),
            limit: MAX_ROW_HEIGHT_RANGES,
        });
    }

    ranges.sort_by_key(|r| r.min);
    for pair in ranges.windows(2) {
        let (prev, next) = (&pair[0], &pair[1]);
        if prev.max >= next.min {
            return Err(Error::InvalidRowHeightRange {
                min: next.min,
                max: next.max,
                reason: "overlaps another row height range".to_string(),
            });
        }
    }

    sheet.set_row_heights(ranges, default_row_height);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SheetVisibility;

    fn range(min: u32, max: u32, height_pt: f64) -> RowHeightRange {
        RowHeightRange {
            min,
            max,
            height_pt,
        }
    }

    fn new_sheet() -> Sheet {
        Sheet::new("Sheet1".into(), SheetVisibility::Visible)
    }

    #[test]
    fn registers_non_overlapping_ranges_in_sorted_order() {
        let mut sheet = new_sheet();
        resolve(
            &mut sheet,
            vec![range(10, 20, 20.0), range(1, 5, 10.0)],
            None,
        )
        .unwrap();

        assert_eq!(sheet.row_height(1), Some(10.0));
        assert_eq!(sheet.row_height(10), Some(20.0));
        assert_eq!(sheet.row_height(6), None);
    }

    #[test]
    fn overlapping_ranges_are_an_error() {
        let mut sheet = new_sheet();
        let err = resolve(
            &mut sheet,
            vec![range(1, 10, 10.0), range(5, 15, 20.0)],
            None,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidRowHeightRange { .. }));
        assert_eq!(sheet.row_height(7), None);
    }

    #[test]
    fn touching_ranges_are_not_overlapping() {
        let mut sheet = new_sheet();
        resolve(&mut sheet, vec![range(1, 2, 10.0), range(3, 4, 20.0)], None).unwrap();
        assert_eq!(sheet.row_height(2), Some(10.0));
        assert_eq!(sheet.row_height(3), Some(20.0));
    }

    #[test]
    fn identical_duplicate_ranges_are_an_error() {
        let mut sheet = new_sheet();
        let err =
            resolve(&mut sheet, vec![range(1, 5, 10.0), range(1, 5, 10.0)], None).unwrap_err();
        assert!(matches!(err, Error::InvalidRowHeightRange { .. }));
    }

    #[test]
    fn range_count_at_the_limit_is_accepted() {
        let mut sheet = new_sheet();
        let ranges: Vec<RowHeightRange> = (1..=MAX_ROW_HEIGHT_RANGES as u32)
            .map(|i| range(i, i, 15.0))
            .collect();
        resolve(&mut sheet, ranges, None).unwrap();
        assert_eq!(sheet.row_height(1), Some(15.0));
        assert_eq!(sheet.row_height(MAX_ROW_HEIGHT_RANGES as u32), Some(15.0));
    }

    #[test]
    fn range_count_over_the_limit_is_too_many_row_height_ranges() {
        let mut sheet = new_sheet();
        let ranges: Vec<RowHeightRange> = (1..=(MAX_ROW_HEIGHT_RANGES as u32 + 1))
            .map(|i| range(i, i, 15.0))
            .collect();
        let err = resolve(&mut sheet, ranges, None).unwrap_err();
        assert!(matches!(
            err,
            Error::TooManyRowHeightRanges {
                count,
                limit
            } if count == MAX_ROW_HEIGHT_RANGES + 1 && limit == MAX_ROW_HEIGHT_RANGES
        ));
    }

    #[test]
    fn empty_range_list_still_registers_default_row_height() {
        let mut sheet = new_sheet();
        resolve(&mut sheet, vec![], Some(15.0)).unwrap();
        assert_eq!(sheet.row_height(1), Some(15.0));
    }

    #[test]
    fn no_ranges_and_no_default_is_none() {
        let mut sheet = new_sheet();
        resolve(&mut sheet, vec![], None).unwrap();
        assert_eq!(sheet.row_height(1), None);
    }
}
