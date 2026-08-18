// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Phase 4: validates the `<cols>` range list before registering it with
//! `Sheet::set_col_widths`.

use crate::error::Error;
use crate::model::{ColWidthRange, Sheet};

/// Cap on the number of `<col>` range entries accepted for a single sheet.
///
/// A minimal `<col min="1" max="1" width=".."/>` entry is only ~40-50
/// bytes, so the Zip Bomb byte-size cap (512 MiB by default) alone permits
/// well over ten million of them — an amount that would still cost real
/// CPU time (the O(R log R) sort below) and memory (a `Vec<ColWidthRange>`
/// entry per range) even though this validation, unlike
/// `resolve::merge::MAX_MERGE_REGIONS`, has no O(R^2)/O(R^3) risk of its
/// own to guard against. This cap keeps R itself bounded independently of
/// byte size — the same reasoning as `MAX_MERGE_REGIONS` — while leaving
/// ample headroom over the handful of ranges a real-world sheet typically
/// has (Issue #39).
pub(crate) const MAX_COLUMN_WIDTH_RANGES: usize = 2_000;

/// Validates `ranges` and registers them into `sheet`, along with
/// `default_col_width` (from `<sheetFormatPr defaultColWidth="..">`, if
/// present).
///
/// Rejects a batch larger than [`MAX_COLUMN_WIDTH_RANGES`] as
/// `Error::TooManyColumnWidthRanges` before doing any sorting. Otherwise
/// rejects any individual range with `min > max` (mirrors
/// `resolve::merge::validate_region`'s reversed start/end check — without
/// this, a range like `min=10, max=5` isn't a crash or memory-safety
/// issue, since `Sheet::column_width`'s binary search simply never
/// matches it for any column, but it would silently register as dead,
/// unreachable data instead of surfacing the malformed input as an error;
/// found via PR #48 review), then sorts `ranges` by `min` — O(R log R) —
/// and rejects any two adjacent ranges that overlap
/// (`Error::InvalidColumnWidthRange`), fail-closed (nothing is registered
/// if any range is invalid). Checking only adjacent pairs after sorting by
/// `min` is sufficient to detect *any* overlapping pair: if every adjacent
/// pair satisfies `prev.max < next.min`, that relation chains transitively
/// across the whole sorted sequence, so no non-adjacent pair can overlap
/// either.
pub(crate) fn resolve(
    sheet: &mut Sheet,
    mut ranges: Vec<ColWidthRange>,
    default_col_width: Option<f64>,
) -> Result<(), Error> {
    if ranges.len() > MAX_COLUMN_WIDTH_RANGES {
        return Err(Error::TooManyColumnWidthRanges {
            count: ranges.len(),
            limit: MAX_COLUMN_WIDTH_RANGES,
        });
    }

    for range in &ranges {
        if range.min > range.max {
            return Err(Error::InvalidColumnWidthRange {
                min: range.min,
                max: range.max,
                reason: "min must not be greater than max".to_string(),
            });
        }
    }

    ranges.sort_by_key(|r| r.min);
    for pair in ranges.windows(2) {
        let (prev, next) = (&pair[0], &pair[1]);
        if prev.max >= next.min {
            return Err(Error::InvalidColumnWidthRange {
                min: next.min,
                max: next.max,
                reason: "overlaps another column width range".to_string(),
            });
        }
    }

    sheet.set_col_widths(ranges, default_col_width);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SheetVisibility;

    fn range(min: u32, max: u32, width: f64) -> ColWidthRange {
        ColWidthRange { min, max, width }
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

        assert_eq!(sheet.column_width(1), Some(10.0));
        assert_eq!(sheet.column_width(10), Some(20.0));
        assert_eq!(
            sheet.col_width_ranges(),
            &[range(1, 5, 10.0), range(10, 20, 20.0)]
        );
    }

    #[test]
    fn reversed_min_max_is_an_error() {
        // PR #48 review: without this check, a range like min=10, max=5
        // isn't a crash (column_width's binary search just never matches
        // it), but it would silently register as dead, unreachable data
        // instead of surfacing the malformed input as an error.
        let mut sheet = new_sheet();
        let err = resolve(&mut sheet, vec![range(10, 5, 99.0)], None).unwrap_err();
        assert!(matches!(err, Error::InvalidColumnWidthRange { .. }));
        assert_eq!(sheet.col_width_ranges().len(), 0);
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
        assert!(matches!(err, Error::InvalidColumnWidthRange { .. }));
        assert_eq!(sheet.col_width_ranges().len(), 0);
    }

    #[test]
    fn touching_ranges_are_not_overlapping() {
        // A1:B (max=2) followed immediately by C: (min=3) — adjacent
        // columns, never actually overlapping.
        let mut sheet = new_sheet();
        resolve(&mut sheet, vec![range(1, 2, 10.0), range(3, 4, 20.0)], None).unwrap();
        assert_eq!(sheet.column_width(2), Some(10.0));
        assert_eq!(sheet.column_width(3), Some(20.0));
    }

    #[test]
    fn identical_duplicate_ranges_are_an_error() {
        let mut sheet = new_sheet();
        let err =
            resolve(&mut sheet, vec![range(1, 5, 10.0), range(1, 5, 10.0)], None).unwrap_err();
        assert!(matches!(err, Error::InvalidColumnWidthRange { .. }));
    }

    #[test]
    fn range_count_at_the_limit_is_accepted() {
        let mut sheet = new_sheet();
        let ranges: Vec<ColWidthRange> = (1..=MAX_COLUMN_WIDTH_RANGES as u32)
            .map(|i| range(i, i, 1.0))
            .collect();
        resolve(&mut sheet, ranges, None).unwrap();
        assert_eq!(sheet.col_width_ranges().len(), MAX_COLUMN_WIDTH_RANGES);
    }

    #[test]
    fn range_count_over_the_limit_is_too_many_column_width_ranges() {
        let mut sheet = new_sheet();
        let ranges: Vec<ColWidthRange> = (1..=(MAX_COLUMN_WIDTH_RANGES as u32 + 1))
            .map(|i| range(i, i, 1.0))
            .collect();
        let err = resolve(&mut sheet, ranges, None).unwrap_err();
        assert!(matches!(
            err,
            Error::TooManyColumnWidthRanges {
                count,
                limit
            } if count == MAX_COLUMN_WIDTH_RANGES + 1 && limit == MAX_COLUMN_WIDTH_RANGES
        ));
    }

    #[test]
    fn empty_range_list_still_registers_default_col_width() {
        let mut sheet = new_sheet();
        resolve(&mut sheet, vec![], Some(9.0)).unwrap();
        assert_eq!(sheet.column_width(1), Some(9.0));
        assert_eq!(sheet.default_col_width(), Some(9.0));
    }

    #[test]
    fn a_full_width_single_range_does_not_expand_into_per_column_entries() {
        // The realistic worst case: one <col min="1" max="16384" .../>
        // covering the whole sheet. Must register as exactly one range,
        // not 16,384 (same principle as MergedRegion's huge-region test).
        let mut sheet = new_sheet();
        resolve(&mut sheet, vec![range(1, 16_384, 8.43)], None).unwrap();
        assert_eq!(sheet.col_width_ranges().len(), 1);
        assert_eq!(sheet.column_width(1), Some(8.43));
        assert_eq!(sheet.column_width(16_384), Some(8.43));
    }
}
