# `resolve/mod.rs` Design Doc

*[日本語](mod.md)*

Design doc for `src/resolve/mod.rs`. This is the entry point for Phase 4 (analysis and deferred resolution) as defined by [architecture.md](../architecture.en.md), and is the aggregation file that orchestrates the resolution steps in [shared_strings.md](shared_strings.en.md), [merge.md](merge.en.md), and [style.md](style.en.md) on a per-sheet basis. Satisfying architecture.md's design principle 2 ("`resolve/` has no dependency on I/O or XML structure and works entirely with in-memory data structures such as `model::Sheet`") is a hard requirement for this file and all of its submodules.

## Responsibility / Scope

- Declares submodules (`mod shared_strings; mod merge; mod style; mod column_width; mod color;`) and re-exports public types
- Provides the entry function `resolve_sheet`, which takes one sheet's worth of unresolved data (the `model::Sheet` built by Phase 3, the pending lists of shared-string indices / style IDs, the `<cols>` range list plus `defaultColWidth`, and the `<mergeCells>` range list) and invokes the resolution steps in the order [shared_strings.md](shared_strings.en.md) → [style.md](style.en.md) → [column_width.md](column_width.en.md) → [merge.md](merge.en.md)
- Re-exports [`resolve/color.rs`](color.en.md) (`resolve_color`. Issue #76) so it can be called directly from outside the crate — unlike the four steps up to [column_width.md](column_width.en.md), `resolve_sheet` itself never calls it (see Dependencies below)
- **Not responsible for**: the resolution logic itself (looking up shared-string indices, applying styles, validating/registering column-width ranges or merged ranges, or resolving a color, are each submodule's responsibility), XML parsing itself (`parse/worksheet.rs`, etc.), building `SharedStringTable` / `StyleSheet` (`parse/shared_strings.rs` / `parse/styles.rs`)

## Key Types / Functions (draft)

```rust
mod shared_strings;
mod merge;
mod style;
mod column_width;
mod color;

pub use color::resolve_color;

use crate::error::Error;
use crate::model::sheet::{ColWidthRange, MergedRegion, Sheet};
use crate::model::style::StyleSheet;
// PendingSharedString/PendingStyle are Phase 3's own output data, so
// parse/worksheet.rs defines them (reflects the PR #9 review — see
// Dependencies).
use crate::parse::worksheet::{PendingSharedString, PendingStyle};

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
    col_width_ranges: Vec<ColWidthRange>,
    default_col_width: Option<f64>,
    merge_regions: Vec<MergedRegion>,
) -> Result<(), Error> {
    shared_strings::resolve(sheet, pending_shared_strings, shared_string_table)?;
    style::resolve(sheet, pending_styles, stylesheet)?;
    column_width::resolve(sheet, col_width_ranges, default_col_width)?;
    merge::resolve(sheet, merge_regions)?;
    Ok(())
}
```

## Dependencies

- Depends on: [`resolve/shared_strings.rs`](shared_strings.en.md), [`resolve/merge.rs`](merge.en.md), [`resolve/style.rs`](style.en.md), [`resolve/column_width.rs`](column_width.en.md), [`resolve/color.rs`](color.en.md) (all as `mod` declarations), [`model/sheet.rs`](../model/sheet.en.md) (`Sheet`, `MergedRegion`, `ColWidthRange`), [`error.rs`](../error.en.md). It also depends on [`parse::shared_strings::SharedStringTable`](../parse/shared_strings.en.md) and [`parse::worksheet::{PendingSharedString, PendingStyle}`](../parse/worksheet.en.md), but these are not the kind of "dependency on I/O" that architecture.md design principle 2 forbids; they are dependencies on in-memory structured data that Phase 3 has already built. This therefore does not contradict `resolve/`'s I/O-independence policy (there is no dependency on actual I/O or XML structure such as quick-xml or `std::fs`).
- Depended on by: `pipeline.rs` (calls `resolve_sheet` once Phase 3 completes for each sheet), external callers outside the crate (call the re-exported `resolve_color` directly, combined with `Workbook::theme()`/`ResolvedStyle.fill_fg_color` and similar, at a time of their own choosing — Issue #76 "Option A")

Why `resolve_color` (Issue #76) is never called from `resolve_sheet`: [resolve/color.md](color.en.md)'s "Option A: on-demand resolve API" deliberately decouples color resolution from full-cell traversal (Phase 4), computing it only where a display-oriented caller actually needs it, to avoid any per-cell CPU/memory overhead (see [Issue #76 design proposal](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)). This differs from the other four (`shared_strings`/`style`/`column_width`/`merge`), each of which is mandatory — leaving Phase 3's pending state unresolved would leave cells incomplete — whereas color resolution is an optional, additional conversion performed only when needed, *after* the complete `ColorRef` information already sits on `ResolvedStyle`.

There is no strong ordering requirement among shared-string resolution, style application, and column-width resolution, because each reads/writes independent state. Merge resolution is placed last as a defensive ordering choice: [merge.md](merge.en.md)'s `insert_merge` assumes the origin cell already exists in `cells` at call time, and — since Issue #43 — `Sheet::finalize_merges` (called as the final step of `merge::resolve`) rewrites `cells` to drop every non-origin entry, which must happen only after every other step that might still touch a virtual (non-origin) coordinate has already run.

## Error Handling Policy

- `resolve_sheet` early-returns from each of the three sub-steps via `?`. If any one fails, the subsequent resolution steps do not run (e.g. if shared-string resolution fails, style application and merge resolution are both skipped). By never returning a partially-resolved sheet to the caller, this prevents incomplete data from reaching JSON generation (fail closed).
- The error variants each submodule can return (`Error::SharedStringIndexOutOfBounds` / `Error::InvalidStyleId` / `Error::InvalidMergedRange`) propagate to the caller (`pipeline.rs`) unchanged. This file itself never constructs a new error variant.

## Testing Strategy

- Verify that in the minimal case where all three sub-steps succeed (a sheet containing one shared-string-referencing cell, one style-referencing cell, and one merged range), `resolve_sheet` returns `Ok(())` and every cell ends up resolved as expected (an integration test — exhaustive testing of each sub-step's own logic is each submodule's responsibility)
- Verify that when shared-string resolution fails (out-of-range index), the subsequent style application and merge resolution steps do not run and the error propagates immediately
- Verify that when both the pending lists and the merge-region list are empty (a sheet with only plain numeric/boolean cells), `resolve_sheet` does nothing and returns `Ok(())`

## Open Questions

1. ~~Which module builds `SharedStringTable`~~ → **Resolved**: [`parse/shared_strings.rs`](../parse/shared_strings.en.md) defines and builds `SharedStringTable`. `StyleSheet` / `ResolvedStyle` / `StyleId` were defined preemptively on the [`model/style.rs`](../model/style.en.md) side (addresses PR #8 review feedback), and consistency has now been re-checked against [`parse/styles.rs`](../parse/styles.en.md)'s design.
2. **Validity of the sub-step ordering**: There is currently no strong technical reason for the order "shared-string resolution → style application → merge resolution" beyond the loose requirement that a test should eventually confirm the numFmt date detection in style application does not accidentally overwrite the result of shared-string resolution (`CellValue::Text`). Whether running steps concurrently (not possible under the current design, since it would require simultaneous mutable access to `sheet`) is worthwhile will be reconsidered based on implementation-time profiling.
3. ~~How `pending_shared_strings` / `pending_styles` are passed~~ → **Resolved**: [`parse/worksheet.rs`](../parse/worksheet.en.md) builds them as batched `Vec`s and passes them straight into `resolve_sheet` (following architecture.md's one-directional pipeline policy of "run Phase 4 after Phase 3 completes"). `PendingSharedString`/`PendingStyle`'s own type definitions were relocated to [`parse/worksheet.rs`](../parse/worksheet.en.md) (reflects the [PR #9 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204)), so this file, [`resolve/shared_strings.rs`](shared_strings.en.md), and [`resolve/style.rs`](style.en.md) all uniformly `use` them (resolves [parse/worksheet.md Open Question 1](../parse/worksheet.en.md)).
