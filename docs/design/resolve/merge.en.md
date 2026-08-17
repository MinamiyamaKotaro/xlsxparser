# `resolve/merge.rs` Design Doc

*[日本語](merge.md)*

Design doc for `src/resolve/merge.rs`. This handles the "deferred resolution of merged cells / alias reference mapping" part of Phase 4 as defined by [architecture.md](../architecture.en.md). To realize requirements spec 3.2 (transparent access to merged cells), it validates the `<mergeCells>` merged-range list before registering it with [`model::Sheet::insert_merge`](../model/sheet.en.md).

## Responsibility / Scope

- Takes the `<mergeCells>` merged-range list (`Vec<model::sheet::MergedRegion>`) collected by Phase 3 (`parse/worksheet.rs`), validates each range, and calls `Sheet::insert_merge` in order
- Performs the pre-validation needed to satisfy the contract `Sheet::insert_merge` itself assumes — "the range passed in is valid" ([model/sheet.md Error Handling Policy](../model/sheet.en.md)) — covering overlapping ranges and reversed start/end coordinates
- **Not responsible for**: building the `MergedRegion` itself (converting `start`/`end` via `CellRef::from_a1`) from a `<mergeCells ref="A1:C3">` attribute (`parse/worksheet.rs`, not yet designed — this file assumes it receives a list already converted to `MergedRegion`); the alias-resolution logic for merge origin cells itself (the internals of `model::Sheet::get` / `insert_merge` — see [model/sheet.md](../model/sheet.en.md))

## Key Types / Functions (draft)

```rust
use std::collections::HashSet;

use crate::error::Error;
use crate::model::sheet::{CellRef, MergedRegion, Sheet};

/// Validates `regions` while registering them into `sheet` in order.
/// Call order (from the front of the list) becomes registration order; if
/// multiple ranges contain the same cell, [model/sheet.md](../model/sheet.en.md)
/// Open Question 3's "last write wins" behavior applies as-is. However, this
/// function rejects clear range overlaps (two or more distinct origin cells
/// claiming the same cell) as a validation error, so duplicate registration
/// never actually reaches the `Sheet` side (see Open Question 1).
pub(crate) fn resolve(sheet: &mut Sheet, regions: Vec<MergedRegion>) -> Result<(), Error> {
    let mut occupied: HashSet<CellRef> = HashSet::new();
    for region in &regions {
        validate_region(region, &occupied)?;
        mark_occupied(region, &mut occupied);
    }
    for region in regions {
        sheet.insert_merge(region);
    }
    Ok(())
}

/// Validates that a single merged range is structurally valid (start/end
/// coordinate ordering, overlap with previously registered ranges).
fn validate_region(region: &MergedRegion, occupied: &HashSet<CellRef>) -> Result<(), Error> {
    if region.start.row > region.end.row || region.start.col > region.end.col {
        return Err(Error::InvalidMergedRange {
            start: region.start.to_a1(),
            end: region.end.to_a1(),
            reason: "start must not be greater than end".to_string(),
        });
    }
    for row in region.start.row..=region.end.row {
        for col in region.start.col..=region.end.col {
            if occupied.contains(&CellRef { row, col }) {
                return Err(Error::InvalidMergedRange {
                    start: region.start.to_a1(),
                    end: region.end.to_a1(),
                    reason: "overlaps with another merged range".to_string(),
                });
            }
        }
    }
    Ok(())
}

fn mark_occupied(region: &MergedRegion, occupied: &mut HashSet<CellRef>) {
    for row in region.start.row..=region.end.row {
        for col in region.start.col..=region.end.col {
            occupied.insert(CellRef { row, col });
        }
    }
}
```

## Dependencies

- Depends on: [`model/sheet.rs`](../model/sheet.en.md) (`Sheet::insert_merge`, `MergedRegion`, `CellRef`), [`error.rs`](../error.en.md)
- Depended on by: [`resolve/mod.rs`](mod.en.md) (called from `resolve_sheet`)

Expanding coordinates into a `HashSet<CellRef>` on every call in `validate_region` to detect overlaps carries a computational and memory-efficiency concern when large merged ranges (e.g. hundreds of cells in a grid-paper-style Excel sheet) are numerous (see Open Question 2).

## Error Handling Policy

- Both a range with reversed start/end coordinates and a range overlapping an existing one are rejected as `Error::InvalidMergedRange { start, end, reason }` (directly implementing [model/sheet.md Error Handling Policy](../model/sheet.en.md)'s stated policy of "validating on the `resolve/merge.rs` side before calling `insert_merge`").
- No `panic` (an invalid merged range can stem from untrusted external input, i.e. a malformed `.xlsx`).
- Once validation fails, `resolve` aborts entirely and no further ranges are registered (reject the whole batch if even one is invalid — the same fail-closed principle as `validate_entry_path` in [container/sanitize.md](../container/sanitize.en.md)).

## Testing Strategy

- Verify that multiple non-overlapping merged ranges are correctly registered via `Sheet::insert_merge` (a wiring test confirming `Sheet::get` resolves a virtual cell coordinate to the origin cell)
- Verify that a range with reversed start/end coordinates (e.g. `start: C3, end: A1`) returns `Error::InvalidMergedRange`
- Verify that two merged ranges overlapping even partially (e.g. `A1:C3` and `B2:D4`) return `Error::InvalidMergedRange`
- Verify that when a validation error occurs, no ranges are registered into `Sheet` at all — including ones that passed validation earlier in the list (confirming the whole-batch rejection)
- Verify that an empty merged-range list results in a no-op `Ok(())`
- Verify that a 1x1 merged range (a trivial, effectively-not-merged case) is handled correctly (boundary value)

## Open Questions

1. **Validity of placing overlap validation in this file rather than in `Sheet::insert_merge`**: [model/sheet.md](../model/sheet.en.md) states that "calling `insert_merge` multiple times is expected to simply overwrite," meaning that without this file's validation layer, overlapping ranges would be silently overwritten last-write-wins. Introducing validation turns this behavior into an error — a design decision — but it's undecided whether the requirements spec includes a future use case that intentionally wants to tolerate overlaps (e.g. an error-tolerant mode that reads as much of a corrupted `.xlsx` as possible).
2. **Computational complexity of overlap detection**: The current approach of expanding coordinates into a `HashSet<CellRef>` costs O(cells in the merged range) in both memory and time. This could become a problem for extremely large merged ranges (an extreme case like A1:XFD1048576), so whether a more efficient data structure such as an interval tree is needed will be decided after validation against real data.
3. **Validity of passing `MergedRegion` as a batched `Vec`**: As with [resolve/mod.md Open Question 3](mod.en.md), since `parse/worksheet.rs` is not yet designed, at what point in the stream the `<mergeCells>` element (which typically appears near the end of `worksheet.xml`, after all row data) can be finalized into a `Vec<MergedRegion>` is undecided and will be settled when `parse/worksheet.rs` is designed.
