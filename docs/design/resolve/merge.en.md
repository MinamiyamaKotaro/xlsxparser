# `resolve/merge.rs` Design Doc

*[日本語](merge.md)*

Design doc for `src/resolve/merge.rs`. This handles the "deferred resolution of merged cells / alias reference mapping" part of Phase 4 as defined by [architecture.md](../architecture.en.md). To realize requirements spec 3.2 (transparent access to merged cells), it validates the `<mergeCells>` merged-range list before registering it with [`model::Sheet::insert_merge`](../model/sheet.en.md).

## Responsibility / Scope

- Takes the `<mergeCells>` merged-range list (`Vec<model::sheet::MergedRegion>`) collected by Phase 3 (`parse/worksheet.rs`), validates each range, and calls `Sheet::insert_merge` in order
- Performs the pre-validation needed to satisfy the contract `Sheet::insert_merge` itself assumes — "the range passed in is valid" ([model/sheet.md Error Handling Policy](../model/sheet.en.md)) — covering overlapping ranges and reversed start/end coordinates
- **Not responsible for**: building the `MergedRegion` itself (converting `start`/`end` via `CellRef::from_a1`) from a `<mergeCells ref="A1:C3">` attribute (`parse/worksheet.rs`, not yet designed — this file assumes it receives a list already converted to `MergedRegion`); the alias-resolution logic for merge origin cells itself (the internals of `model::Sheet::get` / `insert_merge` — see [model/sheet.md](../model/sheet.en.md))

## Key Types / Functions (draft)

```rust
use crate::error::Error;
use crate::model::sheet::{MergedRegion, Sheet};

/// Validates `regions` while registering them into `sheet` in order.
/// Call order (from the front of the list) becomes registration order; if
/// multiple ranges contain the same cell, [model/sheet.md](../model/sheet.en.md)
/// Open Question 3's "last write wins" behavior applies as-is. However, this
/// function rejects clear range overlaps (two or more distinct origin cells
/// claiming the same cell) as a validation error, so duplicate registration
/// never actually reaches the `Sheet` side (see Open Question 1).
pub(crate) fn resolve(sheet: &mut Sheet, regions: Vec<MergedRegion>) -> Result<(), Error> {
    let mut accepted: Vec<MergedRegion> = Vec::with_capacity(regions.len());
    for region in &regions {
        validate_region(region, &accepted)?;
        accepted.push(*region);
    }
    for region in regions {
        sheet.insert_merge(region);
    }
    Ok(())
}

/// Validates that a single merged range is structurally valid (start/end
/// coordinate ordering, overlap with ranges that already passed validation).
///
/// Overlap detection does not expand ranges into individual cells; instead
/// it performs an O(1) geometric intersection test against each element of
/// `accepted` (O(number already validated) per call), so that even a huge
/// merged range (e.g. `A1:XFD1048576`) never incurs cost proportional to
/// its cell count (over a billion cells) (addresses PR #8 review feedback,
/// resolving Open Question 2).
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
```

## Dependencies

- Depends on: [`model/sheet.rs`](../model/sheet.en.md) (`Sheet::insert_merge`, `MergedRegion`), [`error.rs`](../error.en.md)
- Depended on by: [`resolve/mod.rs`](mod.en.md) (called from `resolve_sheet`)

`validate_region` never expands a range into a `HashSet<CellRef>`; it detects overlaps purely through rectangle-intersection tests against ranges that already passed validation (`accepted: &[MergedRegion]`). The per-range cost is O(1) regardless of the range's area (cell count), and the total cost of validating `N` ranges is bounded by O(N²) (each range is compared against those already validated) (addresses PR #8 review feedback — the earlier `HashSet<CellRef>` expansion could turn a single huge range like `A1:XFD1048576` into a loop over more than a billion cells, capable of hanging the CPU).

## Error Handling Policy

- Both a range with reversed start/end coordinates and a range overlapping an existing one are rejected as `Error::InvalidMergedRange { start, end, reason }` (directly implementing [model/sheet.md Error Handling Policy](../model/sheet.en.md)'s stated policy of "validating on the `resolve/merge.rs` side before calling `insert_merge`").
- No `panic` (an invalid merged range can stem from untrusted external input, i.e. a malformed `.xlsx`).
- Once validation fails, `resolve` aborts entirely and no further ranges are registered (reject the whole batch if even one is invalid — the same fail-closed principle as `validate_entry_path` in [container/sanitize.md](../container/sanitize.en.md)).

## Testing Strategy

- Verify that multiple non-overlapping merged ranges are correctly registered via `Sheet::insert_merge` (a wiring test confirming `Sheet::get` resolves a virtual cell coordinate to the origin cell)
- Verify that a range with reversed start/end coordinates (e.g. `start: C3, end: A1`) returns `Error::InvalidMergedRange`
- Verify that two merged ranges overlapping even partially (e.g. `A1:C3` and `B2:D4`) return `Error::InvalidMergedRange`
- Verify that two merged ranges that are close on the coordinate axes but never actually overlap (e.g. `A1:B2` and `C1:D2`, merely adjacent columns) are not mistakenly flagged as overlapping (a boundary-value test for `regions_overlap`)
- **Verify that validating a single extremely large merged range (e.g. `A1:XFD1048576`) completes immediately, without cost proportional to its cell count** (a regression test for the DoS resilience raised in the PR #8 review)
- Verify that when a validation error occurs, no ranges are registered into `Sheet` at all — including ones that passed validation earlier in the list (confirming the whole-batch rejection)
- Verify that an empty merged-range list results in a no-op `Ok(())`
- Verify that a 1x1 merged range (a trivial, effectively-not-merged case) is handled correctly (boundary value)

## Open Questions

1. **Validity of placing overlap validation in this file rather than in `Sheet::insert_merge`**: [model/sheet.md](../model/sheet.en.md) states that "calling `insert_merge` multiple times is expected to simply overwrite," meaning that without this file's validation layer, overlapping ranges would be silently overwritten last-write-wins. Introducing validation turns this behavior into an error — a design decision — but it's undecided whether the requirements spec includes a future use case that intentionally wants to tolerate overlaps (e.g. an error-tolerant mode that reads as much of a corrupted `.xlsx` as possible).
2. ~~Computational complexity of overlap detection~~ → **Resolved**: replaced the per-cell `HashSet<CellRef>` expansion with an O(1) geometric intersection test (separating-axis test) between rectangles. Validating one new range against N already-validated ranges costs O(N), and O(N²) overall — independent of a range's area (cell count). If the number of ranges N becomes very large (e.g. tens of thousands), there is room to further improve to O(N log N) via sorting plus a sweep line, but since it is expected to be rare for a real-world Excel file to reach tens of thousands of merged ranges, the simple O(N²) implementation is considered sufficient for now (addresses PR #8 review feedback).
3. **Validity of passing `MergedRegion` as a batched `Vec`**: As with [resolve/mod.md Open Question 3](mod.en.md), since `parse/worksheet.rs` is not yet designed, at what point in the stream the `<mergeCells>` element (which typically appears near the end of `worksheet.xml`, after all row data) can be finalized into a `Vec<MergedRegion>` is undecided and will be settled when `parse/worksheet.rs` is designed.
