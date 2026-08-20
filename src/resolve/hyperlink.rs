// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Phase 4 (partial): validates `<hyperlink>` ranges (Issue #95) before
//! registering them with `Sheet::finalize_hyperlinks`. Mirrors
//! `resolve::merge`'s shape almost exactly — overlap validation, then a
//! sweep-line resolution pass — since both are "a batch of rectangular
//! ranges keyed by nothing durable, resolved against already-populated
//! cells" problems. The rest of hyperlink resolution (`r:id` -> raw
//! Target string) needs ZIP I/O against the worksheet's own `_rels`, so —
//! like Issue #65's image resolution — that part lives in `pipeline.rs`
//! rather than here (architecture.md design principle 2: `resolve/`
//! stays I/O-independent).

use crate::error::Error;
use crate::model::{HyperlinkRange, Sheet};

/// Cap on the number of `<hyperlink>` entries accepted for a single
/// sheet — deliberately reuses `resolve::merge::MAX_MERGE_REGIONS`'s
/// exact value and reasoning rather than deriving an independent one:
/// `validate_range` below is the same O(N^2) shape as `resolve::merge`'s
/// `validate_region` (each new range checked against every already-
/// accepted one), so the identical cost curve applies (security review
/// `docs/security/code-review.md` Finding 1's measurement — ~424ms at
/// N=40,000, ~10s extrapolated at N=194,000 — governs this cap too,
/// without needing to re-derive it).
pub(crate) const MAX_HYPERLINKS_PER_SHEET: usize = 20_000;

/// Validates `ranges`, then registers the whole batch into `sheet` in one
/// call to `Sheet::finalize_hyperlinks`. Unlike `resolve::merge::resolve`
/// (which calls `Sheet::insert_merge` once per region, then
/// `Sheet::finalize_merges` once at the end), there is no per-range
/// `Sheet` call here — `finalize_hyperlinks` both backfills each range's
/// placeholder cell and runs the sweep in one pass, since there is no
/// reason to expose the pre-sweep state to any other caller.
pub(crate) fn resolve(sheet: &mut Sheet, ranges: Vec<HyperlinkRange>) -> Result<(), Error> {
    if ranges.len() > MAX_HYPERLINKS_PER_SHEET {
        return Err(Error::TooManyHyperlinks {
            count: ranges.len(),
            limit: MAX_HYPERLINKS_PER_SHEET,
        });
    }
    let mut accepted: Vec<&HyperlinkRange> = Vec::with_capacity(ranges.len());
    for range in &ranges {
        validate_range(range, &accepted)?;
        accepted.push(range);
    }
    sheet.finalize_hyperlinks(ranges);
    Ok(())
}

/// Validates a single hyperlink range's start/end ordering and its
/// disjointness from every range already validated. Overlap detection is
/// the same O(1)-per-pair separating-axis test `resolve::merge`'s
/// `regions_overlap` uses — never expanded into per-cell comparisons, so
/// even a huge range (`A1:XFD1048576`) costs the same as a 1x1 one.
///
/// Only hyperlink-range-vs-hyperlink-range overlap is checked. A
/// hyperlink range overlapping a `MergedRegion` is fine and expected —
/// merges and hyperlinks are independent OOXML concepts occupying the
/// same coordinate space, and nothing here needs them mutually
/// exclusive.
fn validate_range(range: &HyperlinkRange, accepted: &[&HyperlinkRange]) -> Result<(), Error> {
    if range.start.row > range.end.row || range.start.col > range.end.col {
        return Err(Error::InvalidHyperlinkRange {
            start: range.start.to_a1(),
            end: range.end.to_a1(),
            reason: "start must not be greater than end".to_string(),
        });
    }
    for other in accepted {
        if ranges_overlap(range, other) {
            return Err(Error::InvalidHyperlinkRange {
                start: range.start.to_a1(),
                end: range.end.to_a1(),
                reason: format!(
                    "overlaps with another hyperlink range ({}:{})",
                    other.start.to_a1(),
                    other.end.to_a1()
                ),
            });
        }
    }
    Ok(())
}

/// Determines in O(1) whether two rectangular hyperlink ranges overlap
/// (separating-axis test — identical shape to `resolve::merge`'s
/// `regions_overlap`).
fn ranges_overlap(a: &HyperlinkRange, b: &HyperlinkRange) -> bool {
    a.start.row <= b.end.row
        && a.end.row >= b.start.row
        && a.start.col <= b.end.col
        && a.end.col >= b.start.col
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CellRef, Hyperlink, SheetVisibility};

    fn r(row: u32, col: u32) -> CellRef {
        CellRef { row, col }
    }

    fn range(start: (u32, u32), end: (u32, u32), target: &str) -> HyperlinkRange {
        HyperlinkRange {
            start: r(start.0, start.1),
            end: r(end.0, end.1),
            hyperlink: Hyperlink {
                target: Some(target.to_string()),
                location: None,
                tooltip: None,
            },
        }
    }

    #[test]
    fn registers_non_overlapping_ranges_and_resolves_every_covered_cell() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        // (2,2)/(2,3) must already be populated cells for the range
        // (2,1):(2,3) to reach them — only a range's own origin
        // ((2,1) here) is backfilled automatically (see
        // Sheet::finalize_hyperlinks's doc comment for why).
        sheet.insert_cell(
            r(2, 2),
            crate::model::Cell {
                value: Some(crate::model::CellValue::Number(1.0)),
                style: None,
            },
        );
        sheet.insert_cell(
            r(2, 3),
            crate::model::Cell {
                value: Some(crate::model::CellValue::Number(2.0)),
                style: None,
            },
        );
        let ranges = vec![
            range((1, 1), (1, 1), "https://a.example/"),
            range((2, 1), (2, 3), "https://b.example/"),
        ];
        resolve(&mut sheet, ranges).unwrap();

        assert_eq!(
            sheet.hyperlink_at(r(1, 1)).unwrap().target.as_deref(),
            Some("https://a.example/")
        );
        for col in 1..=3 {
            assert_eq!(
                sheet.hyperlink_at(r(2, col)).unwrap().target.as_deref(),
                Some("https://b.example/"),
                "col {col} must independently carry the range's hyperlink"
            );
        }
    }

    #[test]
    fn fully_blank_cell_in_range_other_than_origin_is_not_resolved() {
        // Pins down the accepted limitation (docs/design/resolve/hyperlink.en.md
        // Open Questions / Sheet::finalize_hyperlinks doc comment): only a
        // range's own origin is backfilled, so a cell elsewhere in the
        // range that was never independently populated stays invisible.
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        resolve(&mut sheet, vec![range((1, 1), (1, 3), "a")]).unwrap();

        assert!(sheet.hyperlink_at(r(1, 1)).is_some());
        assert!(sheet.hyperlink_at(r(1, 2)).is_none());
        assert!(sheet.get(r(1, 2)).is_none());
    }

    #[test]
    fn reversed_start_end_is_an_error() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let err = resolve(&mut sheet, vec![range((3, 3), (1, 1), "x")]).unwrap_err();
        assert!(matches!(err, Error::InvalidHyperlinkRange { .. }));
    }

    #[test]
    fn overlapping_ranges_are_an_error() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let ranges = vec![range((1, 1), (3, 3), "a"), range((2, 2), (4, 4), "b")];
        let err = resolve(&mut sheet, ranges).unwrap_err();
        assert!(matches!(err, Error::InvalidHyperlinkRange { .. }));
    }

    #[test]
    fn adjacent_non_overlapping_ranges_are_not_flagged() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let ranges = vec![range((1, 1), (2, 2), "a"), range((1, 3), (2, 4), "b")];
        resolve(&mut sheet, ranges).unwrap();
    }

    #[test]
    fn overlapping_a_merged_region_is_accepted() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_merge(crate::model::MergedRegion {
            start: r(1, 1),
            end: r(3, 3),
        });
        resolve(&mut sheet, vec![range((1, 1), (2, 2), "a")]).unwrap();
        assert_eq!(
            sheet.hyperlink_at(r(1, 1)).unwrap().target.as_deref(),
            Some("a")
        );
    }

    #[test]
    fn huge_range_validates_without_cell_count_cost() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        resolve(&mut sheet, vec![range((1, 1), (1_048_576, 16_384), "a")]).unwrap();
    }

    #[test]
    fn validation_failure_registers_nothing() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let ranges = vec![range((1, 1), (2, 2), "a"), range((1, 1), (3, 3), "b")];
        let err = resolve(&mut sheet, ranges).unwrap_err();
        assert!(matches!(err, Error::InvalidHyperlinkRange { .. }));
        assert!(sheet.hyperlink_at(r(1, 1)).is_none());
        assert!(sheet.get(r(1, 1)).is_none());
    }

    #[test]
    fn empty_range_list_is_a_no_op() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        resolve(&mut sheet, vec![]).unwrap();
    }

    #[test]
    fn range_count_at_the_limit_is_accepted() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let ranges: Vec<HyperlinkRange> = (1..=MAX_HYPERLINKS_PER_SHEET as u32)
            .map(|row| range((row, 1), (row, 1), "a"))
            .collect();
        resolve(&mut sheet, ranges).unwrap();
    }

    #[test]
    fn range_count_over_the_limit_is_too_many_hyperlinks() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let ranges: Vec<HyperlinkRange> = (1..=(MAX_HYPERLINKS_PER_SHEET as u32 + 1))
            .map(|row| range((row, 1), (row, 1), "a"))
            .collect();
        let err = resolve(&mut sheet, ranges).unwrap_err();
        assert!(matches!(
            err,
            Error::TooManyHyperlinks { count, limit }
            if count == MAX_HYPERLINKS_PER_SHEET + 1 && limit == MAX_HYPERLINKS_PER_SHEET
        ));
    }

    #[test]
    fn ranges_overlap_boundary_values() {
        assert!(ranges_overlap(
            &range((1, 1), (3, 3), "a"),
            &range((3, 3), (5, 5), "b")
        ));
        assert!(!ranges_overlap(
            &range((1, 1), (2, 2), "a"),
            &range((1, 3), (2, 4), "b")
        ));
    }
}
