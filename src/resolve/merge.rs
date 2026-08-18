// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Phase 4: validates the `<mergeCells>` range list before registering it
//! with `Sheet::insert_merge`.

use crate::error::Error;
use crate::model::{MergedRegion, Sheet};

/// Cap on the number of `<mergeCell>` entries accepted for a single sheet.
///
/// `validate_region` checks each new region against every region already
/// accepted, so the whole batch costs O(N^2) in the number of regions —
/// each `<mergeCell ref="A1:B1"/>` entry is only ~20-30 bytes, so the Zip
/// Bomb byte-size cap (512 MiB by default) alone permits well over 10
/// million of them, which is already an effectively-unbounded amount of
/// O(N^2) work. This cap keeps the worst case bounded to roughly a few
/// hundred milliseconds instead (security review
/// `docs/security/code-review.md` Finding 1, which measured this scaling
/// directly: N=40,000 took ~424ms, and N=194,000 was already extrapolated
/// to ~10s). 20,000 leaves ample headroom over the tens-to-hundreds of
/// merged regions a real-world sheet typically has.
pub(crate) const MAX_MERGE_REGIONS: usize = 20_000;

/// Validates `regions` while registering them into `sheet` in order.
/// Rejects a batch larger than [`MAX_MERGE_REGIONS`] as
/// `Error::TooManyMergedRanges` before doing any O(N^2) work, a reversed
/// start/end coordinate pair or a range that overlaps one already
/// validated as `Error::InvalidMergedRange`. If even one range is invalid,
/// nothing is registered (reject the whole batch — fail closed).
///
/// Once every region is registered, calls `Sheet::finalize_merges` to
/// batch-resolve every cell to its merge origin in one pass — this is what
/// keeps `json.rs`'s later `iter_cells` call fast regardless of how the
/// merges are arranged (Issue #43).
pub(crate) fn resolve(sheet: &mut Sheet, regions: Vec<MergedRegion>) -> Result<(), Error> {
    if regions.len() > MAX_MERGE_REGIONS {
        return Err(Error::TooManyMergedRanges {
            count: regions.len(),
            limit: MAX_MERGE_REGIONS,
        });
    }
    let mut accepted: Vec<MergedRegion> = Vec::with_capacity(regions.len());
    for region in &regions {
        validate_region(region, &accepted)?;
        accepted.push(*region);
    }
    for region in regions {
        sheet.insert_merge(region);
    }
    sheet.finalize_merges();
    Ok(())
}

/// Validates that a single merged range is structurally valid (start/end
/// coordinate ordering, overlap with ranges that already passed
/// validation).
///
/// Overlap detection does not expand ranges into individual cells; instead
/// it performs an O(1) geometric intersection test against each element of
/// `accepted` (O(number already validated) per call), so that even a huge
/// merged range (e.g. `A1:XFD1048576`) never incurs cost proportional to
/// its cell count (over a billion cells).
fn validate_region(region: &MergedRegion, accepted: &[MergedRegion]) -> Result<(), Error> {
    if region.start.row > region.end.row || region.start.col > region.end.col {
        return Err(Error::InvalidMergedRange {
            start: region.start.to_a1(),
            end: region.end.to_a1(),
            reason: "start must not be greater than end".to_string(),
        });
    }
    for other in accepted {
        if regions_overlap(region, other) {
            return Err(Error::InvalidMergedRange {
                start: region.start.to_a1(),
                end: region.end.to_a1(),
                reason: "overlaps with another merged range".to_string(),
            });
        }
    }
    Ok(())
}

/// Determines in O(1) whether two rectangular ranges (merged regions)
/// overlap on both axes (separating-axis test: if either axis is fully
/// disjoint, the rectangles don't overlap).
fn regions_overlap(a: &MergedRegion, b: &MergedRegion) -> bool {
    a.start.row <= b.end.row
        && a.end.row >= b.start.row
        && a.start.col <= b.end.col
        && a.end.col >= b.start.col
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CellRef, SheetVisibility};

    fn region(start: (u32, u32), end: (u32, u32)) -> MergedRegion {
        MergedRegion {
            start: CellRef {
                row: start.0,
                col: start.1,
            },
            end: CellRef {
                row: end.0,
                col: end.1,
            },
        }
    }

    #[test]
    fn registers_non_overlapping_regions() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let regions = vec![region((1, 1), (2, 2)), region((3, 1), (4, 2))];
        resolve(&mut sheet, regions).unwrap();

        assert_eq!(
            sheet.get(CellRef { row: 2, col: 2 }),
            sheet.get(CellRef { row: 1, col: 1 })
        );
        assert_eq!(
            sheet.get(CellRef { row: 4, col: 2 }),
            sheet.get(CellRef { row: 3, col: 1 })
        );
    }

    #[test]
    fn reversed_start_end_is_an_error() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = resolve(&mut sheet, vec![region((3, 3), (1, 1))]).unwrap_err();
        assert!(matches!(err, Error::InvalidMergedRange { .. }));
    }

    #[test]
    fn overlapping_regions_are_an_error() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let regions = vec![region((1, 1), (3, 3)), region((2, 2), (4, 4))];
        let err = resolve(&mut sheet, regions).unwrap_err();
        assert!(matches!(err, Error::InvalidMergedRange { .. }));
    }

    #[test]
    fn adjacent_non_overlapping_regions_are_not_flagged() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        // A1:B2 and C1:D2 — adjacent columns, never actually overlapping.
        let regions = vec![region((1, 1), (2, 2)), region((1, 3), (2, 4))];
        resolve(&mut sheet, regions).unwrap();
    }

    #[test]
    fn huge_region_validates_without_cell_count_cost() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        resolve(&mut sheet, vec![region((1, 1), (1_048_576, 16_384))]).unwrap();
    }

    #[test]
    fn validation_failure_registers_nothing() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let regions = vec![region((1, 1), (2, 2)), region((1, 1), (3, 3))];
        let err = resolve(&mut sheet, regions).unwrap_err();
        assert!(matches!(err, Error::InvalidMergedRange { .. }));
        // The first (valid) region must not have been registered either.
        assert!(sheet.get(CellRef { row: 1, col: 1 }).is_none());
        assert!(sheet.merged_region_at(CellRef { row: 1, col: 1 }).is_none());
    }

    #[test]
    fn empty_region_list_is_a_no_op() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        resolve(&mut sheet, vec![]).unwrap();
    }

    #[test]
    fn region_count_at_the_limit_is_accepted() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let regions: Vec<MergedRegion> = (1..=MAX_MERGE_REGIONS as u32)
            .map(|row| region((row, 1), (row, 2)))
            .collect();
        resolve(&mut sheet, regions).unwrap();
    }

    #[test]
    fn region_count_over_the_limit_is_too_many_merged_ranges() {
        // Security review docs/security/code-review.md Finding 1: without
        // this cap, validating N regions costs O(N^2) (each new region is
        // checked against every already-accepted one), so a file with a
        // few hundred KB of `<mergeCell>` entries could block the caller
        // for minutes. The identical/overlapping regions here are
        // irrelevant — the count check must reject the batch before any
        // O(N^2) validation work ever runs.
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let regions = vec![region((1, 1), (1, 1)); MAX_MERGE_REGIONS + 1];
        let err = resolve(&mut sheet, regions).unwrap_err();
        assert!(matches!(
            err,
            Error::TooManyMergedRanges {
                count,
                limit
            } if count == MAX_MERGE_REGIONS + 1 && limit == MAX_MERGE_REGIONS
        ));
        assert!(sheet.get(CellRef { row: 1, col: 1 }).is_none());
    }

    #[test]
    fn one_by_one_region_is_handled() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        resolve(&mut sheet, vec![region((1, 1), (1, 1))]).unwrap();
        assert!(sheet.get(CellRef { row: 1, col: 1 }).is_some());
    }

    #[test]
    fn regions_overlap_boundary_values() {
        assert!(regions_overlap(
            &region((1, 1), (3, 3)),
            &region((3, 3), (5, 5))
        )); // touching corner counts as overlap
        assert!(!regions_overlap(
            &region((1, 1), (2, 2)),
            &region((1, 3), (2, 4))
        ));
    }
}
