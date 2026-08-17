# `model/sheet.rs` Design Doc

*[日本語](sheet.md)*

Design doc for `src/model/sheet.rs`. Using `Cell` / `CellRef` from [model/cell.md](cell.en.md), this defines `Sheet`, the sparse matrix representing a single sheet's worth of data. This is the core module that realizes requirements spec 3.1 (memory optimization via a sparse matrix) and 3.2 (transparent access to merged cells) as types.

## Responsibility / Scope

- Defines `Sheet`, a sparse matrix that holds only cells with data or formatting, backed by `HashMap<CellRef, Cell>`
- Holds the "virtual cell coordinate → origin cell" alias reference mapping for merged cells, enabling transparent access via `get()`
- **Not responsible for**: parsing `<mergeCells>` XML (`parse/worksheet.rs`), or the logic that matches merge ranges against cell data to build the alias mapping itself (`resolve/merge.rs` — this file only provides a data structure that holds and looks up an already-built mapping)

## Key Types (draft)

```rust
use std::collections::HashMap;
use crate::model::cell::{Cell, CellRef};

/// A merged range. Holds the top-left (origin cell) and bottom-right coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergedRegion {
    pub start: CellRef, // origin cell (holds the actual data)
    pub end: CellRef,
}

impl MergedRegion {
    pub fn row_span(&self) -> u32 { self.end.row - self.start.row + 1 }
    pub fn col_span(&self) -> u32 { self.end.col - self.start.col + 1 }
}

/// Sparse matrix data for a single sheet.
#[derive(Debug, Clone)]
pub struct Sheet {
    pub name: String,
    cells: HashMap<CellRef, Cell>,
    /// virtual cell coordinate -> origin cell coordinate. Built by resolve/merge.rs.
    merge_aliases: HashMap<CellRef, CellRef>,
    /// Held so json.rs can compute row_span/col_span.
    pub merged_regions: Vec<MergedRegion>,
}

impl Sheet {
    /// Retrieves a cell, resolving the merged-cell alias if needed.
    /// Returns the same `Cell` whether passed the origin or a virtual coordinate.
    pub fn get(&self, r: CellRef) -> Option<&Cell> {
        let origin = self.merge_aliases.get(&r).copied().unwrap_or(r);
        self.cells.get(&origin)
    }

    /// An iterator over origin cells only (for JSON generation).
    pub fn iter_cells(&self) -> impl Iterator<Item = (CellRef, &Cell)>;
}
```

## Dependencies

- Depends on: [`model/cell.rs`](cell.en.md) (`Cell`, `CellRef`)
- Depended on by: `model::Workbook` (holds multiple sheets), `resolve/merge.rs` (builds and writes `merge_aliases` / `merged_regions`), `resolve/shared_strings.rs` / `resolve/style.rs` (rewrite the values in `cells`), `json.rs` (assembles JSON from `iter_cells` and `merged_regions`)

## Error Handling Policy

- `get()` returns `Option` rather than `Result`, since the sparse-matrix nature means a missing cell (i.e. a blank cell) is a normal, expected state.
- Validating invalid merge ranges (overlapping ranges, out-of-range coordinates, etc.) is out of scope for this file; it is handled as an error (the common type in `error.rs`) on the `resolve/merge.rs` side. `Sheet` is a data structure that operates only under the assumption that the mapping has already been built and holds a "trusted state."

## Testing Strategy

- Verifying that `get()` on a blank cell (an unset coordinate) returns `None` (basic sparse-matrix behavior)
- Verifying that `get()` on a virtual coordinate inside a merged range returns the same `Cell` as the origin cell
- Boundary-value tests for `MergedRegion::row_span` / `col_span` (a 1x1 range, a large range)
- Verifying that `iter_cells` returns only origin cells and never includes virtual coordinates

## Open Questions

1. **Managing sheet dimensions (used range)**: For "grid-paper Excel" (extremely many rows/columns), scanning all of `cells` every time to determine the maximum row/column for JSON output or range checks could be costly. It is undecided whether to incrementally track `max_row` / `max_col` on insertion, or to simply trust the `<dimension>` element's value (`worksheet.xml`).
2. **Key type for `cells`**: Whether to adopt `HashMap<CellRef, Cell>` or the `HashMap<(u32, u32), Cell>` shown as an example in the requirements spec. `CellRef` already implements `Hash` so the two are type-equivalent, but which to use is still to be decided from a readability / API-consistency standpoint.
3. **Handling of duplicate/invalid merge ranges**: How `resolve/merge.rs` should behave if a malicious or corrupted XLSX contains overlapping merge ranges (reject with an error, or overwrite on a last-write-wins basis) is undecided. Note that this file's API design (holding `merge_aliases` as a single `HashMap`) assumes "last write wins."
4. **Other `worksheet.xml` metadata such as visibility, frozen rows/columns**: Not explicitly covered by the requirements spec, but if things like `freezePane` or hidden rows/columns are handled in the future, whether to hold them on `Sheet` or split them into a separate type is undecided (currently out of scope and not included in the type).
