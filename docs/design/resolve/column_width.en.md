# `resolve/column_width.rs` Design Doc

*[日本語](column_width.md)*

Design doc for `src/resolve/column_width.rs`. Handles the "column-width range validation and registration" part of Phase 4, added for Issue #39. To realize the downstream "grid-paper Excel" detection use case (see [README.md Motivation](../../../README.md#motivation)), it validates the `<cols>` range list before registering it with [`model::Sheet::set_col_widths`](../model/sheet.en.md).

This file's shape is deliberately a close parallel of [`resolve/merge.rs`](merge.en.md): both validate a batch of ranges collected by Phase 3, then register them into `Sheet` in one call. The two differ where the underlying data differs — see "Relationship to `resolve/merge.rs`" below.

## Responsibility / Scope

- Takes the `<cols>` range list (`Vec<model::sheet::ColWidthRange>`) and `default_col_width: Option<f64>` (from `<sheetFormatPr defaultColWidth>`) collected by Phase 3 (`parse/worksheet.rs`), validates the range list, and calls `Sheet::set_col_widths` once
- Rejects a batch larger than `MAX_COLUMN_WIDTH_RANGES` (`Error::TooManyColumnWidthRanges`) before any sorting, and rejects any two overlapping ranges (`Error::InvalidColumnWidthRange`) after sorting by `min` — fail-closed, mirroring `resolve/merge.rs`'s policy
- **Not responsible for**: building `ColWidthRange` itself from a `<col min=".." max=".." width=".."/>` attribute set (`parse/worksheet.rs`), the binary-search lookup logic itself (`Sheet::column_width` — see [model/sheet.en.md](../model/sheet.en.md))

## Key Types / Functions

```rust
use crate::error::Error;
use crate::model::{ColWidthRange, Sheet};

pub(crate) const MAX_COLUMN_WIDTH_RANGES: usize = 2_000;

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
```

## Relationship to `resolve/merge.rs`

Both files validate-then-register a batch of ranges Phase 3 collected, and both reject the whole batch on the first invalid entry. They differ in validation strategy because their overlap-detection problems have different shapes:

- **`resolve/merge.rs`** validates 2D rectangles (`MergedRegion`, row × column), where checking a new region against every already-accepted one is O(1) per pair via a separating-axis test — but *only* rectangle-vs-rectangle, so the whole batch costs O(N²). A sweep-line rewrite to O(N log N) was considered and explicitly deferred (see [merge.en.md Open Question 2](merge.en.md)); instead a defensive count cap, `MAX_MERGE_REGIONS`, keeps the O(N²) bounded.
- **`resolve/column_width.rs`** validates 1D intervals (`ColWidthRange`, column-only) — a strictly simpler problem. Sorting by `min` once is O(R log R), and checking only *adjacent* pairs after sorting is sufficient to catch every overlapping pair: if every adjacent pair satisfies `prev.max < next.min`, that relation chains transitively across the whole sorted sequence, so no non-adjacent pair can overlap either. This is the O(N log N) sweep-line-shaped approach `resolve/merge.rs` couldn't adopt for its 2D problem, achieved here "for free" because 1D interval overlap reduces directly to a sort.

Even though this module has no O(R²)/O(R³) risk of its own, it still caps the range count (`MAX_COLUMN_WIDTH_RANGES`, 2,000) for the same reason `resolve/merge.rs` does: a minimal `<col min="1" max="1" width=".."/>` entry is only ~40-50 bytes, so the Zip Bomb byte-size cap (512 MiB by default) alone permits well over ten million of them — real CPU time (the sort) and real memory (a `Vec<ColWidthRange>` entry per range) that should be bounded independently of byte size, not because sorting itself is dangerous at that scale, but because "the file format doesn't stop you from doing it" is not the same as "doing it is free."

**Design history**: this reasoning — and the choice not to reuse `resolve/merge.rs`'s O(N²) approach or its `MAX_MERGE_REGIONS` cap value — is the result of five rounds of proposed designs and measured counter-examples in [Issue #36](https://github.com/MinamiyamaKotaro/xlsxparser/issues/36)'s discussion thread: a naive "no overlap handling" approach was measured to risk O(R³) in the worst case; a "last-write-wins" trim/split approach was replaced by outright rejection (matching `resolve/merge.rs`'s own policy) once the trim/split logic's own complexity risk was flagged; the `2,000` cap's justification went through two revisions (first citing an inapplicable O(R²) rationale copied from `MAX_MERGE_REGIONS`'s doc comment, then correctly identifying that the Zip Bomb byte cap alone doesn't bound R, independent of any algorithmic complexity concern) before landing on the reasoning above.

## Dependencies

- Depends on: [`model/sheet.rs`](../model/sheet.en.md) (`Sheet::set_col_widths`, `ColWidthRange`), [`error.rs`](../error.en.md)
- Depended on by: [`resolve/mod.rs`](mod.en.md) (called from `resolve_sheet`, after `style::resolve` and before `merge::resolve`)

## Error Handling Policy

- A range count over `MAX_COLUMN_WIDTH_RANGES`, an individual range with `min > max`, or any two overlapping ranges, is rejected as `Error::TooManyColumnWidthRanges` / `Error::InvalidColumnWidthRange { min, max, reason }` (mirroring `Error::TooManyMergedRanges` / `Error::InvalidMergedRange`'s shape — including `resolve::merge::validate_region`'s reversed-coordinate check, added following [PR #48's review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/48#pullrequestreview-4956349641): a `min > max` range isn't a crash or memory-safety issue on its own (`Sheet::column_width`'s binary search simply never matches it for any column), but without this check it would silently register as dead, unreachable data instead of surfacing the malformed input as an error).
- No `panic` (malformed/adversarial ranges stem from untrusted external input).
- Once validation fails (count, reversed range, or overlap), nothing is registered (fail-closed) — same policy as `resolve/merge.rs`.

## Testing Strategy

- Verify non-overlapping ranges register correctly regardless of input order (sorted internally)
- **Verify a range with `min > max` is rejected as `Error::InvalidColumnWidthRange`, both at this module's level and end to end via a `<col min="10" max="5" .../>` fixture** (PR #48 review finding)
- Verify overlapping ranges (including identical duplicates) are rejected as `Error::InvalidColumnWidthRange`, and that nothing was registered afterward
- Verify touching-but-not-overlapping ranges (`max` of one equals `min - 1` of the next) are accepted — a boundary-value test for the adjacent-pair check
- Verify the range count exactly at `MAX_COLUMN_WIDTH_RANGES` is accepted, and one over it is rejected as `Error::TooManyColumnWidthRanges`
- Verify an empty range list still registers `default_col_width` correctly
- Verify a single full-width range (`min=1, max=16384`) registers as exactly one entry, not one per column (Issue #39's core performance requirement, at this module's level)
- End-to-end: a file with `MAX_COLUMN_WIDTH_RANGES + 1` `<col>` entries is rejected as `Error::TooManyColumnWidthRanges` (`pipeline.rs`'s own test suite, mirroring `excessive_merge_cell_count_is_too_many_merged_ranges`)

## Open Questions

None currently. The core algorithm converged through the Issue #36 review process before implementation began; the one gap found afterward (the missing `min > max` check, [PR #48 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/48#pullrequestreview-4956349641)) is already reflected above rather than left open.
