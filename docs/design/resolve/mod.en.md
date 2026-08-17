# `resolve/mod.rs` Design Doc

*[日本語](mod.md)*

Design doc for `src/resolve/mod.rs`. This is the entry point for Phase 4 (analysis and deferred resolution) as defined by [architecture.md](../architecture.en.md), and is the aggregation file that orchestrates the resolution steps in [shared_strings.md](shared_strings.en.md), [merge.md](merge.en.md), and [style.md](style.en.md) on a per-sheet basis. Satisfying architecture.md's design principle 2 ("`resolve/` has no dependency on I/O or XML structure and works entirely with in-memory data structures such as `model::Sheet`") is a hard requirement for this file and all of its submodules.

## Responsibility / Scope

- Declares submodules (`mod shared_strings; mod merge; mod style;`) and re-exports public types
- Provides the entry function `resolve_sheet`, which takes one sheet's worth of unresolved data (the `model::Sheet` built by Phase 3, plus the pending lists of shared-string indices / style IDs, and the `<mergeCells>` range list) and invokes the resolution steps in the order [shared_strings.md](shared_strings.en.md) → [style.md](style.en.md) → [merge.md](merge.en.md)
- **Not responsible for**: the resolution logic itself (looking up shared-string indices, applying styles, validating/registering merged ranges are each submodule's responsibility), XML parsing itself (`parse/worksheet.rs`, etc.), building `SharedStringTable` / `StyleSheet` (`parse/shared_strings.rs` / `parse/styles.rs`, not yet designed — see Open Question 1)

## Key Types / Functions (draft)

```rust
mod shared_strings;
mod merge;
mod style;

pub use shared_strings::PendingSharedString;
pub use style::{PendingStyle, ResolvedStyle, StyleId, StyleSheet};

use crate::error::Error;
use crate::model::sheet::{MergedRegion, Sheet};

/// Runs Phase 4 resolution over one sheet's worth of unresolved data.
/// Intended to be called once per sheet by `pipeline.rs`.
///
/// Preconditions: `sheet` has already had all cells inserted by Phase 3
/// (`parse/worksheet.rs`). However, cells referencing shared strings are
/// still inserted with `value: None`, and cells referencing styles with
/// `style: None` (see shared_strings.md / style.md for details).
pub fn resolve_sheet(
    sheet: &mut Sheet,
    pending_shared_strings: &[PendingSharedString],
    shared_string_table: &crate::parse::shared_strings::SharedStringTable,
    pending_styles: &[PendingStyle],
    stylesheet: &StyleSheet,
    merge_regions: Vec<MergedRegion>,
) -> Result<(), Error> {
    shared_strings::resolve(sheet, pending_shared_strings, shared_string_table)?;
    style::resolve(sheet, pending_styles, stylesheet)?;
    merge::resolve(sheet, merge_regions)?;
    Ok(())
}
```

## Dependencies

- Depends on: [`resolve/shared_strings.rs`](shared_strings.en.md), [`resolve/merge.rs`](merge.en.md), [`resolve/style.rs`](style.en.md) (all as `mod` declarations), [`model/sheet.rs`](../model/sheet.en.md) (`Sheet`, `MergedRegion`), [`error.rs`](../error.en.md). It also depends on `parse::shared_strings::SharedStringTable` (not yet designed — see Open Question 1), but this is not the kind of "dependency on I/O" that architecture.md design principle 2 forbids; it is a dependency on in-memory structured data that Phase 3 has already built. It therefore does not contradict `resolve/`'s I/O-independence policy (there is no dependency on actual I/O or XML structure such as quick-xml or `std::fs`).
- Depended on by: `pipeline.rs` (calls `resolve_sheet` once Phase 3 completes for each sheet)

There is no strong ordering requirement among the three sub-steps inside `resolve_sheet` (shared-string resolution → style application → merge resolution), because each submodule reads/writes independent cell fields. Merge resolution is placed last as a defensive ordering choice, since [merge.md](merge.en.md)'s `insert_merge` assumes the origin cell already exists in `cells` at call time, and this ordering makes it easier to surface problems early if shared-string or style resolution was missed (see Open Question 2 for details).

## Error Handling Policy

- `resolve_sheet` early-returns from each of the three sub-steps via `?`. If any one fails, the subsequent resolution steps do not run (e.g. if shared-string resolution fails, style application and merge resolution are both skipped). By never returning a partially-resolved sheet to the caller, this prevents incomplete data from reaching JSON generation (fail closed).
- The error variants each submodule can return (`Error::SharedStringIndexOutOfBounds` / `Error::InvalidStyleId` / `Error::InvalidMergedRange`) propagate to the caller (`pipeline.rs`) unchanged. This file itself never constructs a new error variant.

## Testing Strategy

- Verify that in the minimal case where all three sub-steps succeed (a sheet containing one shared-string-referencing cell, one style-referencing cell, and one merged range), `resolve_sheet` returns `Ok(())` and every cell ends up resolved as expected (an integration test — exhaustive testing of each sub-step's own logic is each submodule's responsibility)
- Verify that when shared-string resolution fails (out-of-range index), the subsequent style application and merge resolution steps do not run and the error propagates immediately
- Verify that when both the pending lists and the merge-region list are empty (a sheet with only plain numeric/boolean cells), `resolve_sheet` does nothing and returns `Ok(())`

## Open Questions

1. **Which module builds `SharedStringTable` / `StyleSheet`**: `parse/shared_strings.rs` / `parse/styles.rs` are out of scope for this Issue (only the modules listed in architecture.md) and have not been designed yet. The concrete type and location of `SharedStringTable` (`parse::shared_strings` vs. `resolve::shared_strings`) will be finalized when `parse/shared_strings.rs` is designed. `StyleSheet` / `ResolvedStyle` / `StyleId` were defined preemptively in this document set ([style.md](style.en.md)), but consistency will need to be re-checked when `parse/styles.rs` is designed.
2. **Validity of the sub-step ordering**: There is currently no strong technical reason for the order "shared-string resolution → style application → merge resolution" beyond the loose requirement that a test should eventually confirm the numFmt date detection in style application does not accidentally overwrite the result of shared-string resolution (`CellValue::Text`). Whether running steps concurrently (not possible under the current design, since it would require simultaneous mutable access to `sheet`) is worthwhile will be reconsidered based on implementation-time profiling.
3. **How `pending_shared_strings` / `pending_styles` are passed**: Since `parse/worksheet.rs` is not yet designed, it is undecided whether these lists are passed as a batched `Vec`, or resolved incrementally as part of streaming processing interleaved with `Sheet` construction. The current design assumes batched `Vec` passing, following architecture.md's one-directional pipeline policy of "run Phase 4 after Phase 3 completes."
