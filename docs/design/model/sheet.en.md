# `model/sheet.rs` Design Doc

*[日本語](sheet.md)*

Design doc for `src/model/sheet.rs`. Using `Cell` / `CellRef` from [model/cell.md](cell.en.md), this defines `Sheet`, the sparse matrix representing a single sheet's worth of data. This is the core module that realizes requirements spec 3.1 (memory optimization via a sparse matrix) and 3.2 (transparent access to merged cells) as types.

## Responsibility / Scope

- Defines `Sheet`, a sparse matrix that holds only cells with data or formatting, backed by `HashMap<CellRef, Cell>`
- Resolves a virtual cell coordinate inside a merged region to its origin cell, enabling transparent access via `get()` (see Key Types for how — a bug found at implementation time ruled out the originally-drafted per-cell alias map; see the note right after the code block)
- Keeps `cells` / `merged_regions` fully private, and only allows mutation through a narrow `pub(crate)` API (`insert_cell` / `insert_merge` / `get_mut`), so that `Sheet` itself enforces internal invariants such as keeping `max_row`/`max_col` in sync and backfilling a placeholder for a merge's origin cell
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
    // `start <= end` is a precondition enforced by the caller
    // (`resolve/merge.rs`), not by this type; `row_span`/`col_span` assert it
    // (debug-only) rather than silently underflowing `u32` in release builds
    // (finalized at implementation time — PR #20 review).
    pub fn row_span(&self) -> u32 { debug_assert!(self.start.row <= self.end.row); self.end.row - self.start.row + 1 }
    pub fn col_span(&self) -> u32 { debug_assert!(self.start.col <= self.end.col); self.end.col - self.start.col + 1 }
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
    /// origin cell coordinate -> merged region. Also the sole source of
    /// truth for resolving a virtual coordinate to its origin (via
    /// `resolve_origin`'s geometric containment check) — see the note
    /// below the code block for why this replaced a per-cell alias map.
    merged_regions: HashMap<CellRef, MergedRegion>,
    /// The largest row/column number among inserted cells. Updated incrementally
    /// on each cell insertion; does not depend on the `<dimension>` element's value.
    pub max_row: u32,
    pub max_col: u32,
}

impl Sheet {
    /// Constructs a new, empty sheet. `cells` / `merged_regions` start
    /// empty; `max_row` / `max_col` start at 0. `pipeline.rs` builds one
    /// from [`parse/workbook.rs`](../parse/workbook.en.md)'s result
    /// (`name`/`visibility`) and passes it to
    /// [`parse/worksheet.rs`](../parse/worksheet.en.md) to stream cells into
    /// (see pipeline.md; added after discovering the gap while designing it).
    pub(crate) fn new(name: String, visibility: SheetVisibility) -> Self {
        Self {
            name,
            visibility,
            cells: HashMap::new(),
            merged_regions: HashMap::new(),
            max_row: 0,
            max_col: 0,
        }
    }

    /// Resolves `r` to a merged region's origin coordinate if `r` falls
    /// inside one; otherwise returns `r` unchanged. A linear scan over
    /// `merged_regions`, skipped entirely when there are none (the common
    /// case for a sheet with no merges). Real-world sheets have at most a
    /// few thousand merged regions regardless of sheet dimensions, so this
    /// stays cheap — the same "simple O(N) is fine for expected-small N"
    /// tradeoff `resolve::merge`'s overlap validation already makes.
    fn resolve_origin(&self, r: CellRef) -> CellRef {
        if self.merged_regions.is_empty() {
            return r;
        }
        self.merged_regions
            .values()
            .find(|region| {
                r.row >= region.start.row && r.row <= region.end.row
                    && r.col >= region.start.col && r.col <= region.end.col
            })
            .map_or(r, |region| region.start)
    }

    /// Retrieves a cell, resolving the merged-cell alias if needed.
    /// Returns the same `Cell` whether passed the origin or a virtual coordinate.
    pub fn get(&self, r: CellRef) -> Option<&Cell> {
        let origin = self.resolve_origin(r);
        self.cells.get(&origin)
    }

    /// Retrieves a mutable reference to a cell, resolving the merged-cell alias if
    /// needed. Used by resolve/shared_strings.rs and resolve/style.rs to rewrite a
    /// cell's value/style with resolved data.
    pub(crate) fn get_mut(&mut self, r: CellRef) -> Option<&mut Cell> {
        let origin = self.resolve_origin(r);
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

    /// Registers a merged region, keyed by its origin cell, in
    /// `merged_regions` (membership for any other coordinate in the range
    /// is resolved geometrically on demand by `resolve_origin`, not
    /// precomputed here — see the note below the code block). If the
    /// origin cell does not yet exist in `cells` (a merged range with
    /// neither value nor formatting), a blank placeholder cell (`value:
    /// None`, `style: None`) is inserted first. This guarantees
    /// `iter_cells` always picks up the origin cell, so `json.rs` never
    /// silently drops merge information (including row_span/col_span) for a
    /// fully blank merged range. The region's end coordinate is a virtual
    /// cell that is never inserted into `cells`, so it would never be
    /// reflected in `max_row`/`max_col` via `insert_cell`; it is applied
    /// explicitly here so that a case like "the only real data is at A1,
    /// but it is merged as A1:C3" still expands the sheet's effective used
    /// range.
    pub(crate) fn insert_merge(&mut self, region: MergedRegion) {
        debug_assert!(region.start.row <= region.end.row);
        debug_assert!(region.start.col <= region.end.col);
        if !self.cells.contains_key(&region.start) {
            self.insert_cell(region.start, Cell { value: None, style: None });
        }
        self.merged_regions.insert(region.start, region);
        self.max_row = self.max_row.max(region.end.row);
        self.max_col = self.max_col.max(region.end.col);
    }

    /// Retrieves, in O(1), the merged region an origin cell belongs to
    /// (used by json.rs to compute row_span/col_span).
    pub fn merged_region_at(&self, origin: CellRef) -> Option<&MergedRegion> {
        self.merged_regions.get(&origin)
    }

    /// An iterator over origin cells only (for JSON generation). Excludes
    /// any coordinate that `resolve_origin` maps to a *different* cell:
    /// `parse/worksheet.rs` inserts a `Cell` for every `<c>` element it
    /// streams, including ones inside a merged range that later turn out
    /// not to be the origin (e.g. a virtual cell carrying only border
    /// styling), so `cells` cannot be assumed to hold origin cells
    /// exclusively (fixed at implementation time — PR #20 review;
    /// `cells.iter()` without this filter would leak such virtual cells
    /// into `json.rs`'s output as duplicates).
    pub fn iter_cells(&self) -> impl Iterator<Item = (CellRef, &Cell)> {
        self.cells.iter().filter(|(&r, _)| self.resolve_origin(r) == r).map(|(&r, c)| (r, c))
    }
}
```

**Implementation-time fix: `merge_aliases` removed (a hang bug found while implementing `resolve/`).** The draft above originally had `insert_merge` populate a `merge_aliases: HashMap<CellRef, CellRef>` by iterating every `(row, col)` pair in the region and inserting an alias entry — an O(`row_span * col_span`) loop. That's unbounded for a legitimate full-sheet merge (`A1:XFD1048576`, Excel's actual maximum dimensions, ~17 billion cells), and was found to hang in practice while writing `resolve/merge.rs`'s tests (a merged region built from real worksheet bounds took the test suite well past a two-minute timeout). The fix removes `merge_aliases` entirely; `get`/`get_mut`/`iter_cells` instead resolve membership on demand via `resolve_origin`'s O(N) geometric scan over `merged_regions` (N = number of merged regions on the sheet, not the area of any one of them), which is skipped outright when there are no merges. This trades `get`'s complexity from O(1) to O(N), but N stays small in practice (real spreadsheets have at most a few thousand merged regions regardless of how large any single one is), and it eliminates the hang entirely. `insert_merge` itself is now O(1).

**Follow-up optimization: `merge_bounds` (PR #23 review).** `Sheet` also tracks `merge_bounds: Option<(u32, u32, u32, u32)>` — the union bounding box (min/max row, min/max col) across every merged region's `start`/`end`, updated in `insert_merge` alongside `merged_regions`. `resolve_origin` checks this first: a coordinate outside the combined bounding box is rejected in O(1), before ever touching the O(N) per-region scan. Since most cells on a sheet with merges concentrated in one area fall outside that area entirely, this turns the common case back into O(1) while keeping the O(N) fallback correct for coordinates that land inside the bounding box but between two regions (with a gap between them) rather than inside either one — see the regression test `get_inside_bounding_box_but_outside_any_region_resolves_to_itself`. The bound is a conservative upper bound, not necessarily the tightest possible one: overwriting a merge at the same origin with a smaller region never shrinks `merge_bounds` back down, since the old bound isn't retracted. That only costs a missed early exit in a rare edge case, never a correctness issue, because the full scan remains authoritative whenever the bounds check doesn't reject a coordinate outright.

## Dependencies

- Depends on: [`model/cell.rs`](cell.en.md) (`Cell`, `CellRef`)
- Depended on by: `model::Workbook` (holds multiple sheets), [`pipeline.rs`](../pipeline.en.md) (constructs sheets via `Sheet::new`), `resolve/merge.rs` (calls `insert_merge` to register merged cells), `resolve/shared_strings.rs` / `resolve/style.rs` (rewrite a cell's value/style with resolved data via `get_mut`), [`json.rs`](../json.en.md) (assembles JSON from `iter_cells` and `merged_region_at`), `parse/worksheet.rs` (inserts parsed data via `insert_cell`)

The `cells` / `merged_regions` fields themselves stay fully private — not even `pub(crate)` — and writes to these internal data structures are restricted to the three methods `insert_cell` / `insert_merge` / `get_mut`. The alternative of making the fields directly `pub(crate)` (as originally suggested in review) was also considered, but that would require every caller across multiple `resolve/` modules to individually remember to keep `max_row`/`max_col` up to date and to backfill a merge's origin cell — scattering the invariant across the crate. Restricting writes to these methods keeps the invariant contained inside `Sheet` itself, so callers don't need to worry about correctness.

## Error Handling Policy

- `get()` / `get_mut()` return `Option` rather than `Result`, since the sparse-matrix nature means a missing cell (i.e. a blank cell) is a normal, expected state.
- Validating invalid merge ranges (overlapping ranges, out-of-range coordinates, etc.) is out of scope for this file; it is handled as an error (the common type in `error.rs`) on the `resolve/merge.rs` side, before `insert_merge` is called. `insert_merge` itself operates under the assumption that the range it is given is already valid.

## Testing Strategy

- Verifying that `get()` on a blank cell (an unset coordinate) returns `None` (basic sparse-matrix behavior)
- Verifying that `get()` on a virtual coordinate inside a merged range returns the same `Cell` as the origin cell
- Boundary-value tests for `MergedRegion::row_span` / `col_span` (a 1x1 range, a large range)
- Verifying that `merged_region_at` retrieves the corresponding `MergedRegion` from an origin cell coordinate in O(1) (including behavior on a sheet with many merged regions)
- Verifying that `iter_cells` returns only origin cells and never includes virtual coordinates
- **Verifying that `iter_cells` still excludes a coordinate that already had a `cells` entry (via `insert_cell`) before `insert_merge` made it a virtual coordinate** — the case where `parse/worksheet.rs` streamed a `<c>` element (e.g. border-only styling) for a cell inside a merged range that later turns out not to be the origin (a regression-test point added following the [PR #20 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/20#pullrequestreview-4949786605); without this filter in `iter_cells`, such a cell would leak into `json.rs`'s output as a duplicate of the origin)
- **Verifying that `insert_merge` on a full-sheet-sized region (e.g. `A1:XFD1048576`, Excel's actual maximum dimensions) registers in roughly constant time rather than hanging** (a regression test for the `merge_aliases`-removal fix described after the code block above)
- **Verifying that a coordinate outside every merged region resolves to itself and is unaffected by unrelated regions existing elsewhere on the sheet** (a correctness check for `resolve_origin`'s geometric containment scan)
- **Verifying that a coordinate inside the combined `merge_bounds` box but outside every individual region (i.e. in a gap between two merges) still resolves to itself** — a correctness check specific to the `merge_bounds` O(1) pre-check (PR #23 review): the bounding box being non-rejecting must not shortcut past the authoritative per-region scan
- Verifying that `max_row` / `max_col` are updated correctly on every `insert_cell` call (confirming they can be computed without trusting `<dimension>`)
- **Verifying that calling `insert_merge` on a range with neither value nor formatting inserts a blank placeholder at the origin cell, and that it is then correctly retrievable via `iter_cells` / `merged_region_at`** (a regression-test point added following the [PR #5 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948259819))
- **Verifying that when the only real data is at `A1`, but it is merged as `A1:C3`, calling `insert_merge` results in `max_row == 3` and `max_col == 3`** (regression test for the case where a merge region's end coordinate expands the sheet's effective used range)

## Open Questions

1. ~~Managing sheet dimensions (used range)~~ → **Resolved**: `<dimension>` elements generated by third-party tools can be inaccurate or missing, so they are not trusted. `max_row` / `max_col` are updated incrementally on each cell insertion and exposed as public fields on `Sheet` for O(1) retrieval (finalized following the [PR #5 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948235239)). `insert_merge` updates `max_row`/`max_col` using the region's end coordinate (`region.end`) as well as the origin cell — since the end coordinate is a virtual cell never inserted into `cells`, it is not picked up via `insert_cell` and needs this explicit update (flagged and fixed following [a further review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948277539)).
2. **Key type for `cells`**: Whether to adopt `HashMap<CellRef, Cell>` or the `HashMap<(u32, u32), Cell>` shown as an example in the requirements spec. `CellRef` already implements `Hash` so the two are type-equivalent, but which to use is still to be decided from a readability / API-consistency standpoint.
3. **Handling of duplicate/invalid merge ranges**: How `resolve/merge.rs` should behave if a malicious or corrupted XLSX contains overlapping merge ranges (reject with an error, or overwrite on a last-write-wins basis) is undecided. Note that this file's API design (`insert_merge` called multiple times is assumed to simply overwrite) assumes "last write wins."
4. **Other `worksheet.xml` metadata such as frozen rows/columns**: Not explicitly covered by the requirements spec, but if things like `freezePane` are handled in the future, whether to hold them on `Sheet` or split them into a separate type is undecided (currently out of scope and not included in the type). Visibility is resolved (see Open Question 1 of workbook.md).
5. ~~Crate-internal access to private fields~~ → **Resolved**: rather than making fields like `cells` directly `pub(crate)`, `Sheet` implements the narrow API `insert_cell` / `insert_merge` / `get_mut` and disallows direct access from anywhere else (finalized following the [PR #5 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948259819); see the Dependencies section for the comparison against directly exposing the fields).
