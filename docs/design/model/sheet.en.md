# `model/sheet.rs` Design Doc

*[日本語](sheet.md)*

Design doc for `src/model/sheet.rs`. Using `Cell` / `CellRef` from [model/cell.md](cell.en.md), this defines `Sheet`, the sparse matrix representing a single sheet's worth of data. This is the core module that realizes requirements spec 3.1 (memory optimization via a sparse matrix) and 3.2 (transparent access to merged cells) as types.

## Responsibility / Scope

- Defines `Sheet`, a sparse matrix that holds only cells with data or formatting, backed by `BTreeMap<CellRef, Cell>` (changed from `HashMap` in Issue #87 — see the note right after the code block for details)
- Resolves a virtual cell coordinate inside a merged region to its origin cell, enabling transparent access via `get()` (see Key Types for how — a bug found at implementation time ruled out the originally-drafted per-cell alias map; see the note right after the code block)
- Keeps `cells` / `merged_regions` fully private, and only allows mutation through a narrow `pub(crate)` API (`insert_cell` / `insert_merge` / `get_mut`), so that `Sheet` itself enforces internal invariants such as keeping `max_row`/`max_col` in sync and backfilling a placeholder for a merge's origin cell
- **Not responsible for**: parsing `<mergeCells>` XML (`parse/worksheet.rs`), or the decision logic that matches merge ranges against cell data and calls `insert_merge` (`resolve/merge.rs` — this file only provides the API that safely builds the mapping once called)

## Key Types (draft)

```rust
use std::collections::{BTreeMap, HashMap};
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

/// A cell's hyperlink (Issue #95) — kept raw/unresolved, the same "diff,
/// not display" philosophy `ColorRef` follows (Issue #75): this library
/// never checks a target's existence and never performs an HTTP request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hyperlink {
    pub target: Option<String>,
    pub location: Option<String>,
    pub tooltip: Option<String>,
}

/// A hyperlink's applicable range (Issue #95) — `start`/`end` mirror
/// `MergedRegion`'s shape exactly (a single-cell `ref` has `start ==
/// end`), for the same "hold a range, don't expand it" reason: a
/// `<hyperlink ref="A1:XFD1048576">` must cost O(1), not O(row_span *
/// col_span), the same amplification `insert_merge` already closed off
/// (`insert_merge_on_huge_region_does_not_hang`). `pub(crate)` — unlike
/// `MergedRegion`, never returned to library consumers; `Sheet` only
/// exposes the final per-cell `Hyperlink` via `hyperlink_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HyperlinkRange {
    pub start: CellRef,
    pub end: CellRef,
    pub hyperlink: Hyperlink,
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
    cells: BTreeMap<CellRef, Cell>,
    /// origin cell coordinate -> merged region. Also the sole source of
    /// truth for resolving a virtual coordinate to its origin (via
    /// `resolve_origin`'s geometric containment check) — see the note
    /// below the code block for why this replaced a per-cell alias map.
    merged_regions: HashMap<CellRef, MergedRegion>,
    /// The largest row/column number among inserted cells. Updated incrementally
    /// on each cell insertion; does not depend on the `<dimension>` element's value.
    pub max_row: u32,
    pub max_col: u32,
    /// origin cell coordinate -> hyperlink (Issue #95), populated once by
    /// `finalize_hyperlinks`. `HashMap`, same as `merged_regions` — never
    /// iterated directly to produce output order, only looked up per cell
    /// while walking `iter_cells`'s already-deterministic order.
    hyperlinks: HashMap<CellRef, Hyperlink>,
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
            cells: BTreeMap::new(),
            merged_regions: HashMap::new(),
            max_row: 0,
            max_col: 0,
            hyperlinks: HashMap::new(),
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

    /// Runs once, after every `<mergeCell>` on this sheet has been
    /// registered via `insert_merge` (called by `resolve::merge::resolve`
    /// as its last step). Batch-resolves every currently-inserted cell key
    /// to its merge origin via a sweep line over rows — O((C + M) log
    /// (C + M)) for C cells and M merged regions — then drops every entry
    /// whose key isn't its own origin. See the note after the code block
    /// (Issue #43) for why, and `iter_cells`'s doc comment for what this
    /// buys.
    pub(crate) fn finalize_merges(&mut self) {
        if self.merged_regions.is_empty() {
            return;
        }

        enum SweepEvent {
            Start(CellRef),        // fired at region.start.row
            End(CellRef),          // fired at region.end.row + 1
            Query(CellRef),        // fired at a cell coordinate's row
        }

        let mut events: Vec<(u32, u8, SweepEvent)> = Vec::new();
        for region in self.merged_regions.values() {
            events.push((region.start.row, 0, SweepEvent::Start(region.start)));
            events.push((region.end.row + 1, 0, SweepEvent::End(region.start)));
        }
        for &coord in self.cells.keys() {
            events.push((coord.row, 2, SweepEvent::Query(coord)));
        }
        // End/Start (rank 0/1) before Query at the same row, so a query
        // always sees the fully up-to-date active set for its row.
        events.sort_by_key(|(row, rank, event)| {
            let start_end_rank = match event {
                SweepEvent::End(_) => 0,
                SweepEvent::Start(_) => 1,
                SweepEvent::Query(_) => *rank,
            };
            (*row, start_end_rank)
        });

        // Merges active at the current row, holding each region's `start`
        // (its merged_regions key), sorted by start.col. Column ranges are
        // disjoint by construction (resolve::merge rejects overlaps), so
        // at most one active entry can ever contain a query column.
        let mut active: Vec<CellRef> = Vec::new();
        let mut to_drop: Vec<CellRef> = Vec::new();
        for (_, _, event) in &events {
            match event {
                SweepEvent::Start(start) => {
                    let pos = active.partition_point(|s| s.col < start.col);
                    active.insert(pos, *start);
                }
                SweepEvent::End(start) => {
                    let pos = active.partition_point(|s| s.col < start.col);
                    active.remove(pos);
                }
                SweepEvent::Query(coord) => {
                    let pos = active.partition_point(|s| s.col <= coord.col);
                    if pos == 0 {
                        continue;
                    }
                    let candidate = active[pos - 1];
                    if *coord == candidate {
                        continue; // it's already this region's own origin.
                    }
                    let region = self.merged_regions.get(&candidate).unwrap();
                    if coord.col <= region.end.col {
                        to_drop.push(*coord);
                    }
                }
            }
        }
        for coord in to_drop {
            self.cells.remove(&coord);
        }
    }

    /// An iterator over origin cells only (for JSON generation). No longer
    /// calls `resolve_origin` per cell: `finalize_merges` (called once,
    /// right after every merge is registered) already guarantees every
    /// remaining `cells` key is its own origin, so a plain `cells.iter()`
    /// is correct on its own. See the note after the code block (Issue
    /// #43) for why this changed from the PR #20-era filtered version.
    /// Because `cells` is a `BTreeMap`, this iteration order also follows
    /// `CellRef`'s derived `Ord` (row compared before column), giving a
    /// deterministic row-major, then-column-major order (Issue #87 — see
    /// the note after the code block).
    pub fn iter_cells(&self) -> impl Iterator<Item = (CellRef, &Cell)> {
        self.cells.iter().map(|(&r, c)| (r, c))
    }

    /// Backfills a blank placeholder cell at `r` if none exists yet
    /// (Issue #95) — same reasoning as `insert_merge`'s origin backfill.
    fn backfill_blank_cell(&mut self, r: CellRef) {
        if !self.cells.contains_key(&r) {
            self.insert_cell(r, Cell { value: None, style: None });
        }
    }

    /// Registers every validated hyperlink range at once (called by
    /// `resolve::hyperlink::resolve` after overlap validation — see
    /// resolve/hyperlink.md). Backfills each range's own origin cell, then
    /// resolves every cell key already in `cells` (this backfill included)
    /// to its covering range's `Hyperlink` in a single sweep-line pass —
    /// same Start/End/Query shape as `finalize_merges`, O((C + H) log
    /// (C + H)) for C cells and H ranges, never O(C * H). See the note
    /// after the code block for why a covered cell is keyed by its own
    /// coordinate rather than folded into the range's origin the way a
    /// merge's virtual cells are.
    pub(crate) fn finalize_hyperlinks(&mut self, ranges: Vec<HyperlinkRange>) {
        if ranges.is_empty() {
            return;
        }
        for range in &ranges {
            self.backfill_blank_cell(range.start);
        }

        enum SweepEvent {
            Start(usize), // index into `ranges`
            End(usize),
            Query(CellRef),
        }

        let mut events: Vec<(u32, u8, SweepEvent)> = Vec::new();
        for (i, range) in ranges.iter().enumerate() {
            events.push((range.start.row, 0, SweepEvent::Start(i)));
            events.push((range.end.row + 1, 0, SweepEvent::End(i)));
        }
        for &coord in self.cells.keys() {
            events.push((coord.row, 2, SweepEvent::Query(coord)));
        }
        events.sort_by_key(|(row, kind_rank, event)| {
            let start_end_rank = match event {
                SweepEvent::End(_) => 0,
                SweepEvent::Start(_) => 1,
                SweepEvent::Query(_) => *kind_rank,
            };
            (*row, start_end_rank)
        });

        let mut active: Vec<usize> = Vec::new(); // indices into `ranges`, sorted by start.col
        for (_, _, event) in &events {
            match event {
                SweepEvent::Start(i) => {
                    let col = ranges[*i].start.col;
                    let pos = active.partition_point(|&j| ranges[j].start.col < col);
                    active.insert(pos, *i);
                }
                SweepEvent::End(i) => {
                    let col = ranges[*i].start.col;
                    let pos = active.partition_point(|&j| ranges[j].start.col < col);
                    active.remove(pos);
                }
                SweepEvent::Query(coord) => {
                    let pos = active.partition_point(|&j| ranges[j].start.col <= coord.col);
                    if pos == 0 {
                        continue;
                    }
                    let candidate = active[pos - 1];
                    let range = &ranges[candidate];
                    if coord.col <= range.end.col {
                        self.hyperlinks.insert(*coord, range.hyperlink.clone());
                    }
                }
            }
        }
    }

    /// Retrieves, in O(1), the hyperlink registered at cell `origin`.
    /// Mirrors `merged_region_at`'s convention (no merge-origin resolution
    /// here — `json.rs` only ever calls this with coordinates already
    /// yielded by `iter_cells`).
    pub fn hyperlink_at(&self, origin: CellRef) -> Option<&Hyperlink> {
        self.hyperlinks.get(&origin)
    }
}
```

**Implementation-time fix: `merge_aliases` removed (a hang bug found while implementing `resolve/`).** The draft above originally had `insert_merge` populate a `merge_aliases: HashMap<CellRef, CellRef>` by iterating every `(row, col)` pair in the region and inserting an alias entry — an O(`row_span * col_span`) loop. That's unbounded for a legitimate full-sheet merge (`A1:XFD1048576`, Excel's actual maximum dimensions, ~17 billion cells), and was found to hang in practice while writing `resolve/merge.rs`'s tests (a merged region built from real worksheet bounds took the test suite well past a two-minute timeout). The fix removes `merge_aliases` entirely; `get`/`get_mut`/`iter_cells` instead resolve membership on demand via `resolve_origin`'s O(N) geometric scan over `merged_regions` (N = number of merged regions on the sheet, not the area of any one of them), which is skipped outright when there are no merges. This trades `get`'s complexity from O(1) to O(N), but N stays small in practice (real spreadsheets have at most a few thousand merged regions regardless of how large any single one is), and it eliminates the hang entirely. `insert_merge` itself is now O(1).

**Follow-up optimization: `merge_bounds` (PR #23 review).** `Sheet` also tracks `merge_bounds: Option<(u32, u32, u32, u32)>` — the union bounding box (min/max row, min/max col) across every merged region's `start`/`end`, updated in `insert_merge` alongside `merged_regions`. `resolve_origin` checks this first: a coordinate outside the combined bounding box is rejected in O(1), before ever touching the O(N) per-region scan. Since most cells on a sheet with merges concentrated in one area fall outside that area entirely, this turns the common case back into O(1) while keeping the O(N) fallback correct for coordinates that land inside the bounding box but between two regions (with a gap between them) rather than inside either one — see the regression test `get_inside_bounding_box_but_outside_any_region_resolves_to_itself`. The bound is a conservative upper bound, not necessarily the tightest possible one: overwriting a merge at the same origin with a smaller region never shrinks `merge_bounds` back down, since the old bound isn't retracted. That only costs a missed early exit in a rare edge case, never a correctness issue, because the full scan remains authoritative whenever the bounds check doesn't reject a coordinate outright.

**Fix: `finalize_merges` (Issue #43, closing an adversarial-arrangement gap `merge_bounds` couldn't).** `merge_bounds`'s O(1) pre-check only rejects a coordinate *outside* the combined bounding box; a legitimate arrangement as small as two 1x1 merges at opposite corners of the sheet stretches that box to cover virtually the whole sheet, so every other cell falls back to the O(N) per-region scan regardless of how far it actually is from any merge. `json.rs`'s `iter_cells` calls `resolve_origin` once per cell, so this turned into an O(cells × merged regions) cost during JSON generation — directly measured at tens of seconds of CPU time for a file well within every existing limit (hundreds of thousands of cells, tens of thousands of merges, both within `MAX_MERGE_REGIONS`; a few hundred KB on disk).

Three "clever" per-query fixes were tried and each, in turn, measured to still degrade to roughly the original cost under a specifically-constructed (but entirely legitimate) merge arrangement: a single global "tallest merge seen" cutoff (defeated by one additional full-height merge), fixed-size row bucketing (defeated by concentrating merges and queries into one bucket), and a height-balanced interval tree whose search algorithm explored both children instead of one (defeated by merges that share a wide row range but occupy different columns). See Issue #43's discussion thread for each counter-example and its measurement.

The fix that held up is `Sheet::finalize_merges`: called once by `resolve::merge::resolve`, right after every merge for the sheet has been registered via `insert_merge`. It resolves every currently-inserted cell key to its merge origin in a single sweep-line pass over rows — Start/End events from each merged region's row range, interleaved with a Query event per cell key, sorted once (O((C + M) log (C + M)) for C cells and M merged regions) and then swept in one pass, tracking the column-sorted set of merges active at the current row (disjoint by construction, since `resolve::merge`'s validation already rejects overlapping ranges) — and drops every cell whose key isn't its own origin. This never discards observable data: a coordinate that isn't its own origin was already unreachable through `get`/`iter_cells` (`resolve_origin` always redirects to the origin first), so the entries dropped were already dead, exactly as the `iter_cells_excludes_cells_pre_inserted_at_alias_coordinates` regression test already established. What changes is that `iter_cells` no longer needs to call `resolve_origin` at all afterwards — every remaining key already equals its own origin — closing the cost path regardless of how the merges are arranged in space. `get`/`get_mut`'s own general-purpose fallback (used by external callers querying an arbitrary coordinate after parsing completes) is unchanged; only the internally-driven, file-size-scaled `iter_cells` path — the one an attacker actually controls — needed this.

**Feature: column width (Issue #39).** `Sheet` also holds `col_widths: Vec<ColWidthRange>` (sorted by `min`, mutually non-overlapping) and `default_col_width: Option<f64>`, populated once by `resolve::column_width::resolve` via `Sheet::set_col_widths` — the same "validate-then-register, trust the precondition" split as `resolve::merge`/`insert_merge`. `ColWidthRange { min, max, width }` mirrors `MergedRegion`'s "hold a range, don't expand it" principle: a single `<col min="1" max="16384" .../>` (common in real files) must register as one entry, not 16,384 — see the regression test `a_full_width_single_range_does_not_expand_into_per_column_entries` in `resolve::column_width`.

`column_width(col) -> Option<f64>` binary-searches `col_widths` — `partition_point` for the last range with `min <= col`, then checks whether that range's `max` actually reaches `col` — giving O(log R) regardless of how a file arranges its ranges (R capped at `resolve::column_width::MAX_COLUMN_WIDTH_RANGES`, 2,000). Returns `None` (not a guessed default like Excel's common "Calibri 11 ≈ 8.43 characters") when nothing covers `col` and no `defaultColWidth` was set: that fallback depends on font metrics this library does not compute, so an explicit absence is preferred over a possibly-wrong number. `col_width_ranges()` exposes the raw sorted `Vec` for `json.rs` to serialize as a sheet-level `columns` array — deliberately *not* looked up once per cell and embedded into each cell's JSON object, since a column-level value repeated onto every populated cell in that column would multiply output size for no benefit, working directly against the sparse-output design this library exists for (raised during Issue #36's review discussion, before the column-width work had its own sub-issue).

**Feature: images (Issue #65).** `Sheet` also holds `images: Vec<Image>`, populated once by `pipeline.rs`'s Phase 3.5 via `Sheet::set_images` — the same "resolve elsewhere, register once" split as `set_col_widths`. Unlike `merged_regions`, images are not keyed by any cell coordinate at all: an `ImageAnchor::TwoCell`/`OneCell` marker carries an EMU-unit offset within its cell, so an anchor's position doesn't always align to a cell boundary the way a `MergedRegion`'s does, and there is no single cell an image is naturally "owned by." `images()` exposes the raw `Vec` for `json.rs` to serialize as a sheet-level `images` array, for the same sparse-output reasoning as `col_width_ranges` (see above) — with the added point that there is no cell to attach an image to even if per-cell duplication were otherwise desirable.

**Fix: `cells` changed from `HashMap` to `BTreeMap` (Issue #87).** `iter_cells`'s iteration order feeds `json.rs`'s `cells` array directly, but a `HashMap`'s iteration order depends on a per-process random hash seed (HashDoS resistance), so parsing the same file twice could produce a different cell order in the output JSON each time. This is harmless for use cases that match cells by coordinate `(row, col)`, but a report surfaced a real problem for a *textual* diff (`git diff` or similar) of two JSON snapshots of the same, unchanged file: cells that hadn't actually changed, merely reordered by chance, showed up as a wall of spurious differences. `BTreeMap` iterates in the order of key type `CellRef`'s derived `Ord` (fields compared in declaration order, so `row` before `col`), which gives the row-major, then-column-major order a human reading the JSON would expect, for free, with no separate sort step needed.

Issue #87's PoC (`massive_dense_accounting.xlsx`, 300,000 cells; measured with a custom byte-counting allocator and macOS's `sample` profiler) found:

- **CPU / wall time**: `BTreeMap` is roughly 9-13% faster end to end — counter to `HashMap`'s reputation as the faster structure, but `CellRef` is an 8-byte key, small enough that SipHash's per-key cost turned out to exceed the cost of a B-tree node's handful of integer comparisons.
- **Peak memory during parsing**: `BTreeMap` is roughly 26% better. `HashMap::new()` starts unsized and repeatedly rehashes-and-grows while cells stream in, each time briefly holding both the old and new tables at once — a roughly 54% spike over steady state that `BTreeMap`'s incremental, per-node growth never exhibits.
- **Steady-state memory**: `BTreeMap` is roughly 9.2% worse (about 78.3 bytes/cell vs. `HashMap`'s about 71.7 bytes/cell — node/pointer overhead). Accepted as the trade-off for deterministic output. A separate PoC also confirmed insertion order (ascending/descending/shuffled) doesn't meaningfully move steady-state memory — relevant because `Sheet::insert_cell` is called in the XML's own appearance order by `parse/worksheet.rs`, which is not guaranteed to be row/column-ascending for every real file.

`merged_regions` (an internal-only O(1)/O(N) lookup keyed by origin cell, whose iteration order is never reflected in `json.rs`'s output) is intentionally left as `HashMap` — out of scope for this fix.

**Feature: hyperlinks (Issue #95).** `Sheet` also holds `hyperlinks: HashMap<CellRef, Hyperlink>`, populated once by `finalize_hyperlinks` — called by `resolve::hyperlink::resolve` (see [resolve/hyperlink.md](../resolve/hyperlink.en.md)) after it validates a batch of `HyperlinkRange`s for reversed start/end and mutual overlap. `<hyperlink ref="A1:C3">` (unlike `<mergeCell>`) is not necessarily a merged region — OOXML lets one hyperlink apply to a rectangular selection of otherwise-independent cells, so every covered cell must carry the hyperlink independently in JSON output rather than collapsing into the range's origin the way a merge's virtual cells do.

This ruled out simply reusing `resolve_origin`'s pattern (`get`/`get_mut` resolving a virtual coordinate to one shared origin `Cell`): a first draft did exactly that — geometric bounding-box pre-check, then a linear `.find()` over the range list per query — and turned out to be `resolve_origin`'s *pre-Issue-#43* shape exactly, reintroducing the same O(cells × ranges) cost `finalize_merges`'s sweep-line rewrite eliminated for merges (caught during design review, before implementation — see `resolve/hyperlink.md`'s Testing Strategy for the regression this would otherwise have reproduced). `finalize_hyperlinks` instead runs the same sweep once, but on a match inserts directly into `hyperlinks` keyed by the *query* coordinate (every covered cell independently) rather than dropping non-origin keys the way `finalize_merges` does — the origin cell itself picks up its own entry through the same pass, since `backfill_blank_cell` already guaranteed it exists in `cells` before the sweep runs. (A first pass buffered matches into an intermediate `Vec` before a second loop inserted them into `hyperlinks`, out of an unexamined habit of not mutating `self` mid-sweep; a Copilot PR review comment on PR #96 pointed out nothing in the loop actually borrows `self.hyperlinks` elsewhere — `ranges`/`active`/`events` are all sweep-local — so the insert moved directly into the `Query` arm, dropping the buffer and the second pass entirely.)

Only a range's own origin cell is backfilled (mirrors `insert_merge` exactly) — a cell elsewhere in the range that has no value/style/hyperlink-independent reason to exist stays un-materialized and therefore invisible to `iter_cells`/JSON output, even though Excel would still show it as clickable. Backfilling every covered cell was considered and rejected: it would reopen the same O(row_span * col_span) amplification `insert_merge`'s `merge_aliases` removal (see the note above) closed for merges, since nothing bounds how large a legitimately-`MAX_HYPERLINKS_PER_SHEET`-sized range can be. Accepted as a known limitation (see `resolve/hyperlink.md`'s Open Questions) rather than solved speculatively.

`Sheet::hyperlink_at`, unlike `get`, does not resolve a merge origin — mirrors `merged_region_at`'s convention (`json.rs` only ever calls either with a coordinate `iter_cells` already yielded, which is always an origin by construction).

## Dependencies

- Depends on: [`model/cell.rs`](cell.en.md) (`Cell`, `CellRef`)
- Depended on by: `model::Workbook` (holds multiple sheets), [`pipeline.rs`](../pipeline.en.md) (constructs sheets via `Sheet::new`; Phase 3.5 calls `set_images` — see [parse/drawing.md](../parse/drawing.en.md)), `resolve/merge.rs` (calls `insert_merge` to register merged cells, then `finalize_merges` once all of them are registered), `resolve/shared_strings.rs` / `resolve/style.rs` (rewrite a cell's value/style with resolved data via `get_mut`), `resolve/column_width.rs` (calls `set_col_widths` once validated), [`resolve/hyperlink.rs`](../resolve/hyperlink.en.md) (calls `finalize_hyperlinks` once, after validating a batch), [`json.rs`](../json.en.md) (assembles JSON from `iter_cells`, `merged_region_at`, `col_width_ranges`, `default_col_width`, `images`, `hyperlink_at`), `parse/worksheet.rs` (inserts parsed data via `insert_cell`)

The `cells` / `merged_regions` fields themselves stay fully private — not even `pub(crate)` — and writes to these internal data structures are restricted to `insert_cell` / `insert_merge` / `get_mut` / `finalize_merges`. The alternative of making the fields directly `pub(crate)` (as originally suggested in review) was also considered, but that would require every caller across multiple `resolve/` modules to individually remember to keep `max_row`/`max_col` up to date and to backfill a merge's origin cell — scattering the invariant across the crate. Restricting writes to these methods keeps the invariant contained inside `Sheet` itself, so callers don't need to worry about correctness.

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
- **Verifying `finalize_merges` is a no-op on a sheet with no merges** (the common case must stay cheap)
- **Verifying `finalize_merges` drops a cell that was pre-inserted at a virtual (non-origin) coordinate while keeping both the merge's origin cell and an unrelated standalone cell** (Issue #43; supersedes needing `iter_cells`'s own filter for this)
- **Verifying `finalize_merges` resolves correctly across more than one merged region with disjoint row ranges**, including a data cell that sits outside every region entirely (exercises the sweep line's Start/End bookkeeping, not just a single region)
- **End-to-end regression: a file with `MAX_MERGE_REGIONS` merges arranged to maximize `merge_bounds` (two arranged at opposite corners) plus hundreds of thousands of unrelated cells completes JSON generation without the multi-second stall measured pre-fix** (`tests/security.rs`'s `sparse_merge_bounding_box_does_not_amplify_json_generation_cost`, using the `sparse_merge_bounding_box_amplification` fixture — categorized under Category 5 (security) rather than Category 4 (load), since it's specifically an adversarial-arrangement DoS concern, matching `zip_bomb`/`zip_slip`/`xxe_attack`)
- **`column_width` returns `None` with no ranges and no `defaultColWidth`**, **binary search correctness across multiple ranges** (including boundary values: inside a range, in a gap between ranges, falling back to `defaultColWidth`), **`col_width_ranges`/`default_col_width` expose the raw values for JSON output** (Issue #39; detailed validation lives in `resolve::column_width`'s own test suite)
- **`images()` exposes the raw `Vec` set by `set_images`, unmodified** (Issue #65; per-anchor resolution correctness lives in `parse::drawing`'s and `pipeline.rs`'s own test suites)
- **Verifying `iter_cells`'s iteration order is deterministic — row-major, then column-major, following `CellRef`'s `Ord` — regardless of insertion order** (Issue #87; the guarantee that comes from `cells` being a `BTreeMap`. [`tests/normal.rs`'s `json_cells_array_is_sorted_by_row_then_col_regardless_of_source_order`](../../../tests/normal.rs) uses a fixture whose insertion order is deliberately scrambled, modeling a real file whose XML appearance order is not row/column-ascending)
- **`finalize_hyperlinks` on an empty range list is a no-op** (Issue #95; the common case must stay cheap, mirrors `finalize_merges`)
- **`finalize_hyperlinks` backfills a blank placeholder at a range's origin cell, and it is then retrievable via `hyperlink_at`/`iter_cells`** (mirrors `insert_merge_backfills_blank_origin_cell`)
- **A hyperlink range spanning multiple already-populated cells (not merged) attaches the hyperlink to every one of them independently, not just the origin** — the core correctness requirement `resolve_origin`-style resolution could not satisfy without also reintroducing its pre-#43 cost (see the "Feature: hyperlinks" note above)
- **A fully blank cell inside a hyperlink range, other than the origin, does not appear in `iter_cells`/JSON output** — pins down the accepted limitation described above, so a future change to backfill it is a deliberate decision, not an accidental regression
- **End-to-end regression, same shape as the merge one**: a sheet with `MAX_HYPERLINKS_PER_SHEET` ranges arranged to maximize simultaneous row activity, plus many unrelated cells, completes without cost proportional to cells × ranges (lives in `resolve::hyperlink`'s own test suite via `pipeline.rs` — see [resolve/hyperlink.md](../resolve/hyperlink.en.md))

## Open Questions

1. ~~Managing sheet dimensions (used range)~~ → **Resolved**: `<dimension>` elements generated by third-party tools can be inaccurate or missing, so they are not trusted. `max_row` / `max_col` are updated incrementally on each cell insertion and exposed as public fields on `Sheet` for O(1) retrieval (finalized following the [PR #5 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948235239)). `insert_merge` updates `max_row`/`max_col` using the region's end coordinate (`region.end`) as well as the origin cell — since the end coordinate is a virtual cell never inserted into `cells`, it is not picked up via `insert_cell` and needs this explicit update (flagged and fixed following [a further review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948277539)).
2. **Key type for `cells`**: Whether to adopt `BTreeMap<CellRef, Cell>` or the `HashMap<(u32, u32), Cell>` shown as an example in the requirements spec. `CellRef` already implements both `Hash` and `Ord` so the two are type-equivalent, but which to use is still to be decided from a readability / API-consistency standpoint (the container type itself is resolved separately — see item 6; this item is only about whether the key stays the `CellRef` struct or is flattened to a tuple).
3. **Handling of duplicate/invalid merge ranges**: How `resolve/merge.rs` should behave if a malicious or corrupted XLSX contains overlapping merge ranges (reject with an error, or overwrite on a last-write-wins basis) is undecided. Note that this file's API design (`insert_merge` called multiple times is assumed to simply overwrite) assumes "last write wins."
4. **Other `worksheet.xml` metadata such as frozen rows/columns**: Not explicitly covered by the requirements spec, but if things like `freezePane` are handled in the future, whether to hold them on `Sheet` or split them into a separate type is undecided (currently out of scope and not included in the type). Visibility is resolved (see Open Question 1 of workbook.md).
5. ~~Crate-internal access to private fields~~ → **Resolved**: rather than making fields like `cells` directly `pub(crate)`, `Sheet` implements the narrow API `insert_cell` / `insert_merge` / `get_mut` and disallows direct access from anywhere else (finalized following the [PR #5 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948259819); see the Dependencies section for the comparison against directly exposing the fields).
6. ~~Container type for `cells` (`HashMap` vs. `BTreeMap`)~~ → **Resolved**: adopted `BTreeMap<CellRef, Cell>` (Issue #87). Because `iter_cells`'s iteration order feeds `json.rs`'s `cells` array directly, `HashMap`'s per-process random hash seed meant re-parsing the same file could shuffle unrelated cells in the output, which showed up as spurious noise in a textual diff of two JSON snapshots. PoC measurements (see the note after the code block) confirmed `BTreeMap` doesn't lose on CPU or peak memory either — it's ahead on both — before finalizing the switch. `merged_regions` is out of scope (stays `HashMap`) since its order never reaches JSON output.
7. **Hyperlink range interaction with merged regions, and `finalize_hyperlinks` vs. `finalize_merges` ordering**: see [resolve/hyperlink.md](../resolve/hyperlink.en.md)'s own Open Questions 1-2 — both concern this file's behavior but are tracked there since they're really about `resolve::hyperlink::resolve`'s call-site ordering in `pipeline.rs`, not about `Sheet`'s API surface itself.
