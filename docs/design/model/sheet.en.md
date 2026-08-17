# `model/sheet.rs` Design Doc

*[日本語](sheet.md)*

Design doc for `src/model/sheet.rs`. Using `Cell` / `CellRef` from [model/cell.md](cell.en.md), this defines `Sheet`, the sparse matrix representing a single sheet's worth of data. This is the core module that realizes requirements spec 3.1 (memory optimization via a sparse matrix) and 3.2 (transparent access to merged cells) as types.

## Responsibility / Scope

- Defines `Sheet`, a sparse matrix that holds only cells with data or formatting, backed by `HashMap<CellRef, Cell>`
- Holds the "virtual cell coordinate → origin cell" alias reference mapping for merged cells, enabling transparent access via `get()`
- Keeps `cells` / `merge_aliases` / `merged_regions` fully private, and only allows mutation through a narrow `pub(crate)` API (`insert_cell` / `insert_merge` / `get_mut`), so that `Sheet` itself enforces internal invariants such as keeping `max_row`/`max_col` in sync and backfilling a placeholder for a merge's origin cell
- **Not responsible for**: parsing `<mergeCells>` XML (`parse/worksheet.rs`), or the decision logic that matches merge ranges against cell data and calls `insert_merge` (`resolve/merge.rs` — this file only provides the API that safely builds the mapping once called)

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

/// A sheet's visibility (`workbook.xml`'s `<sheet state="...">`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetVisibility {
    Visible,
    Hidden,
    VeryHidden,
}

/// Sparse matrix data for a single sheet.
#[derive(Debug, Clone)]
pub struct Sheet {
    pub name: String,
    pub visibility: SheetVisibility,
    cells: HashMap<CellRef, Cell>,
    /// virtual cell coordinate -> origin cell coordinate. Built by resolve/merge.rs.
    merge_aliases: HashMap<CellRef, CellRef>,
    /// origin cell coordinate -> merged region. Keying by the origin cell allows
    /// O(1) lookup of row_span/col_span.
    merged_regions: HashMap<CellRef, MergedRegion>,
    /// The largest row/column number among inserted cells. Updated incrementally
    /// on each cell insertion; does not depend on the `<dimension>` element's value.
    pub max_row: u32,
    pub max_col: u32,
}

impl Sheet {
    /// Retrieves a cell, resolving the merged-cell alias if needed.
    /// Returns the same `Cell` whether passed the origin or a virtual coordinate.
    pub fn get(&self, r: CellRef) -> Option<&Cell> {
        let origin = self.merge_aliases.get(&r).copied().unwrap_or(r);
        self.cells.get(&origin)
    }

    /// Retrieves a mutable reference to a cell, resolving the merged-cell alias if
    /// needed. Used by resolve/shared_strings.rs and resolve/style.rs to rewrite a
    /// cell's value/style with resolved data.
    pub(crate) fn get_mut(&mut self, r: CellRef) -> Option<&mut Cell> {
        let origin = self.merge_aliases.get(&r).copied().unwrap_or(r);
        self.cells.get_mut(&origin)
    }

    /// Inserts a cell while updating max_row/max_col at the same time. Writes to
    /// `cells` only ever go through this method, structurally preventing the
    /// dimension fields from going out of sync.
    pub(crate) fn insert_cell(&mut self, r: CellRef, cell: Cell) {
        self.max_row = self.max_row.max(r.row);
        self.max_col = self.max_col.max(r.col);
        self.cells.insert(r, cell);
    }

    /// Registers a merged region: records every coordinate in the range (other than
    /// the origin) as an alias to the origin cell, and records the region itself
    /// keyed by the origin cell in `merged_regions`. If the origin cell does not yet
    /// exist in `cells` (a merged range with neither value nor formatting), a blank
    /// placeholder cell (`value: None`, `style: None`) is inserted first. This
    /// guarantees `iter_cells` always picks up the origin cell, so `json.rs` never
    /// silently drops merge information (including row_span/col_span) for a fully
    /// blank merged range.
    pub(crate) fn insert_merge(&mut self, region: MergedRegion) {
        if !self.cells.contains_key(&region.start) {
            self.insert_cell(region.start, Cell { value: None, style: None });
        }
        for row in region.start.row..=region.end.row {
            for col in region.start.col..=region.end.col {
                let r = CellRef { row, col };
                if r != region.start {
                    self.merge_aliases.insert(r, region.start);
                }
            }
        }
        self.merged_regions.insert(region.start, region);
    }

    /// Retrieves, in O(1), the merged region an origin cell belongs to
    /// (used by json.rs to compute row_span/col_span).
    pub fn merged_region_at(&self, origin: CellRef) -> Option<&MergedRegion> {
        self.merged_regions.get(&origin)
    }

    /// An iterator over origin cells only (for JSON generation).
    pub fn iter_cells(&self) -> impl Iterator<Item = (CellRef, &Cell)>;
}
```

## Dependencies

- Depends on: [`model/cell.rs`](cell.en.md) (`Cell`, `CellRef`)
- Depended on by: `model::Workbook` (holds multiple sheets), `resolve/merge.rs` (calls `insert_merge` to register merged cells), `resolve/shared_strings.rs` / `resolve/style.rs` (rewrite a cell's value/style with resolved data via `get_mut`), `json.rs` (assembles JSON from `iter_cells` and `merged_region_at`), `parse/worksheet.rs` (inserts parsed data via `insert_cell`)

The `cells` / `merge_aliases` / `merged_regions` fields themselves stay fully private — not even `pub(crate)` — and writes to these internal data structures are restricted to the three methods `insert_cell` / `insert_merge` / `get_mut`. The alternative of making the fields directly `pub(crate)` (as originally suggested in review) was also considered, but that would require every caller across multiple `resolve/` modules to individually remember to keep `max_row`/`max_col` up to date and to backfill a merge's origin cell — scattering the invariant across the crate. Restricting writes to these methods keeps the invariant contained inside `Sheet` itself, so callers don't need to worry about correctness.

## Error Handling Policy

- `get()` / `get_mut()` return `Option` rather than `Result`, since the sparse-matrix nature means a missing cell (i.e. a blank cell) is a normal, expected state.
- Validating invalid merge ranges (overlapping ranges, out-of-range coordinates, etc.) is out of scope for this file; it is handled as an error (the common type in `error.rs`) on the `resolve/merge.rs` side, before `insert_merge` is called. `insert_merge` itself operates under the assumption that the range it is given is already valid.

## Testing Strategy

- Verifying that `get()` on a blank cell (an unset coordinate) returns `None` (basic sparse-matrix behavior)
- Verifying that `get()` on a virtual coordinate inside a merged range returns the same `Cell` as the origin cell
- Boundary-value tests for `MergedRegion::row_span` / `col_span` (a 1x1 range, a large range)
- Verifying that `merged_region_at` retrieves the corresponding `MergedRegion` from an origin cell coordinate in O(1) (including behavior on a sheet with many merged regions)
- Verifying that `iter_cells` returns only origin cells and never includes virtual coordinates
- Verifying that `max_row` / `max_col` are updated correctly on every `insert_cell` call (confirming they can be computed without trusting `<dimension>`)
- **Verifying that calling `insert_merge` on a range with neither value nor formatting inserts a blank placeholder at the origin cell, and that it is then correctly retrievable via `iter_cells` / `merged_region_at`** (a regression-test point added in this round)

## Open Questions

1. ~~Managing sheet dimensions (used range)~~ → **Resolved**: `<dimension>` elements generated by third-party tools can be inaccurate or missing, so they are not trusted. `max_row` / `max_col` are updated incrementally on each cell insertion and exposed as public fields on `Sheet` for O(1) retrieval (finalized following the [PR #5 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948235239)).
2. **Key type for `cells`**: Whether to adopt `HashMap<CellRef, Cell>` or the `HashMap<(u32, u32), Cell>` shown as an example in the requirements spec. `CellRef` already implements `Hash` so the two are type-equivalent, but which to use is still to be decided from a readability / API-consistency standpoint.
3. **Handling of duplicate/invalid merge ranges**: How `resolve/merge.rs` should behave if a malicious or corrupted XLSX contains overlapping merge ranges (reject with an error, or overwrite on a last-write-wins basis) is undecided. Note that this file's API design (`insert_merge` called multiple times is assumed to simply overwrite) assumes "last write wins."
4. **Other `worksheet.xml` metadata such as frozen rows/columns**: Not explicitly covered by the requirements spec, but if things like `freezePane` are handled in the future, whether to hold them on `Sheet` or split them into a separate type is undecided (currently out of scope and not included in the type). Visibility is resolved (see Open Question 1 of workbook.md).
5. ~~Crate-internal access to private fields~~ → **Resolved**: rather than making fields like `cells` directly `pub(crate)`, `Sheet` implements the narrow API `insert_cell` / `insert_merge` / `get_mut` and disallows direct access from anywhere else (finalized following the [PR #5 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948259819); see the Dependencies section for the comparison against directly exposing the fields).
