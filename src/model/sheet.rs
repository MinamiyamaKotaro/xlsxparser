// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! `Sheet`: the sparse-matrix data model for a single worksheet, including
//! transparent merged-cell access.

use crate::model::cell::{Cell, CellRef};
use std::collections::{BTreeMap, HashMap};

/// A merged range. Holds the top-left (origin cell) and bottom-right
/// coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergedRegion {
    /// Origin cell (holds the actual data).
    pub start: CellRef,
    pub end: CellRef,
}

impl MergedRegion {
    /// Callers (`resolve/merge.rs`) are expected to validate that `start <=
    /// end` before constructing a region; this is asserted (debug-only) so
    /// a violation is caught during testing rather than silently
    /// underflowing `u32` in release builds.
    pub fn row_span(&self) -> u32 {
        debug_assert!(self.start.row <= self.end.row);
        self.end.row - self.start.row + 1
    }

    pub fn col_span(&self) -> u32 {
        debug_assert!(self.start.col <= self.end.col);
        self.end.col - self.start.col + 1
    }
}

/// A `<col min=".." max=".." width=".."/>` range: column width applies to
/// every column index in `[min, max]` inclusive. Kept as a range rather
/// than expanded per-column, for the same reason `MergedRegion` isn't
/// expanded into per-cell aliases — a single `<col min="1" max="16384"
/// .../>` (common in real files) would otherwise force one entry per
/// column (Issue #39).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColWidthRange {
    pub min: u32,
    pub max: u32,
    pub width: f64,
}

/// A cell-anchored image reference (Issue #65). Holds only the anchor
/// geometry and the resolved target path of the embedded media part — never
/// the image's own bytes, which stay out of scope (a diff-oriented tool has
/// no use for pixel data, and reading it would scale memory use with image
/// count rather than cell count).
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub anchor: ImageAnchor,
    /// Resolved path to the embedded media part (e.g. "xl/media/image1.png").
    pub target: String,
    /// The image's own hyperlink target (`xdr:cNvPr/a:hlinkClick`), if any —
    /// distinct from a cell hyperlink, which this library does not parse.
    /// An `Internal` relationship resolves to a ZIP-entry-name-equivalent
    /// path like `target`; an `External` one keeps the URI verbatim.
    pub hyperlink: Option<String>,
}

/// The two anchor shapes DrawingML defines (`xdr:twoCellAnchor` /
/// `xdr:oneCellAnchor`), modeled as an enum rather than a struct with
/// `Option` fields so an invalid combination (e.g. a `TwoCell` anchor
/// missing its `to` marker) cannot be constructed (Issue #65 design
/// discussion).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageAnchor {
    TwoCell {
        from: AnchorMarker,
        to: AnchorMarker,
    },
    OneCell {
        from: AnchorMarker,
        ext: ImageExtent,
    },
}

/// One `xdr:from`/`xdr:to` marker: a cell plus its EMU-unit offset within
/// that cell. Keeping the offset (rather than rounding to the containing
/// cell, the way `MergedRegion` works) is what lets a diff distinguish an
/// image nudged a few pixels within the same cell from one that hasn't
/// moved at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnchorMarker {
    pub cell: CellRef,
    pub col_off: i64,
    pub row_off: i64,
}

/// `xdr:ext`: an image's width/height in EMU (English Metric Units), used
/// only by `ImageAnchor::OneCell` — a `TwoCell` anchor's size is already
/// implicit in its `from`/`to` markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageExtent {
    pub cx: i64,
    pub cy: i64,
}

/// A cell's hyperlink (`<hyperlinks><hyperlink .../></hyperlinks>`, Issue
/// #95). Kept in raw form — the target/location strings exactly as `_rels`/
/// the XML attribute state them — never validated or resolved, the same
/// "diff, not display" philosophy `ColorRef` already follows (Issue #75):
/// this library never checks that a target URL/part actually exists and
/// never performs an HTTP request, so `target`/`location` can equally
/// describe a link that's since gone dead — which is exactly the kind of
/// change a diff should surface, not hide.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hyperlink {
    /// The relationship's raw Target string, resolved from `r:id` against
    /// the worksheet's own `_rels` (the same map Issue #65's image
    /// resolution already loads) — an external URL verbatim, or an
    /// Internal-relationship-resolved part path. `None` when `<hyperlink>`
    /// has no `r:id` at all (a `location`-only internal jump) or when the
    /// `r:id` doesn't resolve against `_rels` (a malformed file).
    pub target: Option<String>,
    /// `location` attribute verbatim (e.g. `"'Sheet2'!A1"`, a defined
    /// name) — an in-workbook jump target, present alongside or instead of
    /// `target`. Never parsed or validated as a real sheet/cell reference.
    pub location: Option<String>,
    /// `tooltip` attribute verbatim (the ScreenTip text), if present.
    pub tooltip: Option<String>,
}

/// A hyperlink's applicable range (Issue #95) — `start`/`end` mirror
/// `MergedRegion`'s shape exactly (a single-cell `ref` has `start ==
/// end`), for the same "hold a range, don't expand it" reason: a
/// `<hyperlink ref="A1:XFD1048576">` must cost O(1), not O(row_span *
/// col_span), the same amplification `insert_merge`'s `merge_aliases`
/// removal already closed off for merges
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
    /// `BTreeMap` rather than `HashMap` (Issue #87): `iter_cells`'s
    /// iteration order feeds `json.rs`'s `cells` array directly, and a
    /// `HashMap`'s order is randomized per-process (a fresh hash seed each
    /// run, for HashDoS resistance) — harmless for a diff that matches
    /// cells by `(row, col)`, but a real problem for a *textual* diff
    /// (`git diff` or similar) of two JSON snapshots of the same
    /// unchanged file: identical cells reordered by chance looks like a
    /// wall of spurious changes. `BTreeMap`'s key order is `CellRef`'s
    /// derived `Ord` — `row` compared before `col` (declaration order),
    /// giving exactly the row-major, then-column-major order a human
    /// reading the JSON would expect — for free, with no separate sort
    /// step. Measured (PoC on Issue #87, `massive_dense_accounting.xlsx`,
    /// 300,000 cells): ~9-12% faster end to end than `HashMap` despite
    /// `BTreeMap`'s reputation for being the slower of the two — SipHash's
    /// cost per `CellRef` (an 8-byte key) turned out to exceed a B-tree
    /// node's few integer comparisons in practice, and `HashMap::new()`'s
    /// unsized start means repeated rehash-and-grow while streaming in
    /// cells, each temporarily holding old and new tables at once (a
    /// ~54% peak-memory spike over steady state that `BTreeMap`'s
    /// incremental per-node growth doesn't have). The one real cost:
    /// ~9% more steady-state memory per cell (node/pointer overhead) —
    /// accepted as the trade-off for deterministic output.
    cells: BTreeMap<CellRef, Cell>,
    /// origin cell coordinate -> merged region. Also doubles as the source
    /// of truth for resolving a virtual coordinate to its origin (see
    /// `resolve_origin`) via geometric containment, rather than
    /// materializing an alias entry for every cell in the region up front.
    /// An earlier design did the latter (`HashMap<CellRef, CellRef>`,
    /// populated by iterating every row/col in the range inside
    /// `insert_merge`), but that costs O(row_span * col_span) — unbounded
    /// for a legitimate full-sheet merge like `A1:XFD1048576` (~17 billion
    /// cells) — and was found to hang in practice (see the regression test
    /// `insert_merge_on_huge_region_does_not_hang`).
    merged_regions: HashMap<CellRef, MergedRegion>,
    /// Union bounding box (min_row, max_row, min_col, max_col) across every
    /// merged region's `start`/`end`. `None` when there are no merges. Lets
    /// `resolve_origin` reject a coordinate clearly outside every merge in
    /// O(1), before falling back to the O(N) per-region scan — most cells
    /// on a sheet with merges concentrated in one area are outside all of
    /// them (PR #23 review). May end up looser than the tightest possible
    /// box after a same-origin `insert_merge` overwrite shrinks a region
    /// (the old, larger bound is never retracted); that only costs a missed
    /// early exit, never correctness, since the full scan below is still
    /// authoritative.
    merge_bounds: Option<(u32, u32, u32, u32)>,
    /// The largest row/column number among inserted cells. Updated
    /// incrementally on each cell insertion; does not depend on the
    /// `<dimension>` element's value.
    pub max_row: u32,
    pub max_col: u32,
    /// Column-width ranges from `<cols>`, sorted by `min` and mutually
    /// non-overlapping (`resolve::column_width::resolve` guarantees both
    /// before calling `set_col_widths`). Kept as a sorted `Vec` rather
    /// than a `HashMap<u32, f64>` so `column_width` can binary-search in
    /// O(log R) instead of holding one entry per column (Issue #39).
    col_widths: Vec<ColWidthRange>,
    /// `<sheetFormatPr defaultColWidth="..">`, the width for any column not
    /// covered by `col_widths`. `None` when the attribute is absent (Excel
    /// itself falls back to a font-metric-derived default in that case,
    /// which this library does not compute — see `column_width`'s doc).
    default_col_width: Option<f64>,
    /// Images anchored to this sheet (Issue #65). Not keyed by cell — an
    /// anchor's position doesn't always align to a single cell's
    /// boundaries, and unlike a merged region's data, an image isn't
    /// "owned" by whichever cell it happens to overlap.
    images: Vec<Image>,
    /// origin cell coordinate -> hyperlink (Issue #95). `HashMap` rather
    /// than `BTreeMap`, same as `merged_regions`: unlike `cells`, this is
    /// never iterated directly to produce output order — `json.rs` only
    /// ever looks it up per cell, keyed, while walking `iter_cells`'s
    /// already-deterministic `BTreeMap` order, so a `HashMap`'s randomized
    /// iteration order never leaks into the JSON.
    hyperlinks: HashMap<CellRef, Hyperlink>,
}

impl Sheet {
    /// Constructs a new, empty sheet. Called by `pipeline::run` to build
    /// sheets from `parse/workbook.rs`'s output.
    pub(crate) fn new(name: String, visibility: SheetVisibility) -> Self {
        Self {
            name,
            visibility,
            cells: BTreeMap::new(),
            merged_regions: HashMap::new(),
            merge_bounds: None,
            max_row: 0,
            max_col: 0,
            col_widths: Vec::new(),
            default_col_width: None,
            images: Vec::new(),
            hyperlinks: HashMap::new(),
        }
    }

    /// Resolves `r` to a merged region's origin coordinate, if `r` falls
    /// inside one; otherwise returns `r` unchanged. First rejects `r` in
    /// O(1) via `merge_bounds` if it falls outside every merge's combined
    /// bounding box; otherwise falls back to a linear scan over
    /// `merged_regions`. Real-world sheets have at most a few thousand
    /// merged regions regardless of sheet dimensions, so even the fallback
    /// scan stays cheap — the same "simple O(N) is fine for expected-small
    /// N" tradeoff `resolve::merge`'s overlap validation already makes.
    fn resolve_origin(&self, r: CellRef) -> CellRef {
        let Some((min_row, max_row, min_col, max_col)) = self.merge_bounds else {
            return r;
        };
        if r.row < min_row || r.row > max_row || r.col < min_col || r.col > max_col {
            return r;
        }
        self.merged_regions
            .values()
            .find(|region| {
                r.row >= region.start.row
                    && r.row <= region.end.row
                    && r.col >= region.start.col
                    && r.col <= region.end.col
            })
            .map_or(r, |region| region.start)
    }

    /// Retrieves a cell, resolving the merged-cell alias if needed. Returns
    /// the same `Cell` whether passed the origin or a virtual coordinate.
    pub fn get(&self, r: CellRef) -> Option<&Cell> {
        let origin = self.resolve_origin(r);
        self.cells.get(&origin)
    }

    /// Retrieves a mutable reference to a cell, resolving the merged-cell
    /// alias if needed. Used by `resolve/shared_strings.rs` and
    /// `resolve/style.rs` to rewrite a cell's value/style with resolved
    /// data.
    pub(crate) fn get_mut(&mut self, r: CellRef) -> Option<&mut Cell> {
        let origin = self.resolve_origin(r);
        self.cells.get_mut(&origin)
    }

    /// Inserts a cell while updating max_row/max_col at the same time.
    /// Writes to `cells` only ever go through this method, structurally
    /// preventing the dimension fields from going out of sync.
    pub(crate) fn insert_cell(&mut self, r: CellRef, cell: Cell) {
        self.max_row = self.max_row.max(r.row);
        self.max_col = self.max_col.max(r.col);
        self.cells.insert(r, cell);
    }

    /// Registers a merged region, keyed by its origin cell, in
    /// `merged_regions` (`get`/`get_mut`/`iter_cells` resolve membership
    /// geometrically via `resolve_origin` — see that method and the
    /// `merged_regions` field doc for why no per-cell alias is
    /// materialized here). If the origin cell does not yet exist in
    /// `cells` (a merged range with neither value nor formatting), a blank
    /// placeholder cell is inserted first, so `iter_cells` always picks up
    /// the origin cell. Calling this again for the same origin overwrites
    /// the previous region (last-write-wins).
    pub(crate) fn insert_merge(&mut self, region: MergedRegion) {
        debug_assert!(region.start.row <= region.end.row);
        debug_assert!(region.start.col <= region.end.col);

        if !self.cells.contains_key(&region.start) {
            self.insert_cell(
                region.start,
                Cell {
                    value: None,
                    style: None,
                },
            );
        }
        self.merged_regions.insert(region.start, region);
        let (min_row, max_row, min_col, max_col) = self.merge_bounds.unwrap_or((
            region.start.row,
            region.end.row,
            region.start.col,
            region.end.col,
        ));
        self.merge_bounds = Some((
            min_row.min(region.start.row),
            max_row.max(region.end.row),
            min_col.min(region.start.col),
            max_col.max(region.end.col),
        ));
        self.max_row = self.max_row.max(region.end.row);
        self.max_col = self.max_col.max(region.end.col);
    }

    /// Retrieves, in O(1), the merged region an origin cell belongs to (used
    /// by `json.rs` to compute row_span/col_span).
    pub fn merged_region_at(&self, origin: CellRef) -> Option<&MergedRegion> {
        self.merged_regions.get(&origin)
    }

    /// Runs once, after every `<mergeCell>` on this sheet has been
    /// registered via `insert_merge` (called by `resolve::merge::resolve`
    /// as its last step). Batch-resolves every currently-inserted cell key
    /// to its merge origin via a single sweep over rows, then drops every
    /// entry whose key isn't its own origin.
    ///
    /// This never discards observable data: a coordinate that isn't its
    /// own origin was already unreachable through `get`/`get_mut`/
    /// `iter_cells` (`resolve_origin` always redirects to the origin
    /// first — see `iter_cells_excludes_cells_pre_inserted_at_alias_coordinates`),
    /// so every entry this drops was already dead. What changes is that
    /// `iter_cells` no longer needs to call `resolve_origin` per cell
    /// afterwards (see its doc comment).
    ///
    /// Before this existed, a sheet whose merges maximized `merge_bounds`
    /// (e.g. two 1x1 merges at opposite corners) forced every
    /// `iter_cells` call to fall back to the O(number of merged regions)
    /// scan in `resolve_origin`, for every cell — a legitimate file
    /// (hundreds of thousands of cells, tens of thousands of merges, both
    /// within existing limits) could cost tens of seconds of CPU time in
    /// `json.rs`'s output generation (measured; see Issue #43). Sorting
    /// events once and sweeping is O((C + M) log (C + M)) for C cells and
    /// M merged regions, independent of how the merges are arranged in
    /// space — closing the adversarial-arrangement gap that three earlier,
    /// more "clever" attempts (a global tallest-region cutoff, fixed-size
    /// row bucketing, an augmented BST with ad hoc pruning) were each
    /// found, by direct measurement, not to close (see Issue #43's
    /// discussion for the counter-examples).
    pub(crate) fn finalize_merges(&mut self) {
        if self.merged_regions.is_empty() {
            return;
        }

        enum SweepEvent {
            /// Fired at `region.start.row`.
            Start(CellRef),
            /// Fired at `region.end.row + 1`, i.e. one past the last row
            /// the region is active for.
            End(CellRef),
            /// Fired at the row of a cell coordinate to resolve.
            Query(CellRef),
        }

        let mut events: Vec<(u32, u8, SweepEvent)> =
            Vec::with_capacity(self.merged_regions.len() * 2 + self.cells.len());
        for region in self.merged_regions.values() {
            events.push((region.start.row, 0, SweepEvent::Start(region.start)));
            events.push((region.end.row + 1, 0, SweepEvent::End(region.start)));
        }
        for &coord in self.cells.keys() {
            events.push((coord.row, 2, SweepEvent::Query(coord)));
        }
        // Tie-break: at the same row, End/Start (rank 0/1 below) must be
        // applied before any Query at that row sees the active set.
        events.sort_by_key(|(row, kind_rank, event)| {
            let start_end_rank = match event {
                SweepEvent::End(_) => 0,
                SweepEvent::Start(_) => 1,
                SweepEvent::Query(_) => *kind_rank,
            };
            (*row, start_end_rank)
        });

        // Merges active at the current sweep row, holding each region's
        // `start` (its key in `merged_regions`), sorted by `start.col`.
        // Disjoint column ranges are guaranteed by resolve::merge's
        // overlap validation, so at most one active entry can ever
        // contain a given query column.
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
                    debug_assert_eq!(active.get(pos), Some(start));
                    active.remove(pos);
                }
                SweepEvent::Query(coord) => {
                    let pos = active.partition_point(|s| s.col <= coord.col);
                    if pos == 0 {
                        continue;
                    }
                    let candidate_start = active[pos - 1];
                    if *coord == candidate_start {
                        continue; // it's already this region's own origin.
                    }
                    let region = self
                        .merged_regions
                        .get(&candidate_start)
                        .expect("active set only ever holds keys currently in merged_regions");
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
    /// filters via `resolve_origin`: `finalize_merges` (called once, right
    /// after every merge is registered — see its doc comment) already
    /// guarantees every remaining `cells` key is its own origin, so a
    /// plain `cells.iter()` is correct on its own and doesn't pay
    /// `resolve_origin`'s per-cell cost (Issue #43).
    pub fn iter_cells(&self) -> impl Iterator<Item = (CellRef, &Cell)> {
        self.cells.iter().map(|(&r, c)| (r, c))
    }

    /// Registers this sheet's column-width ranges. Called once by
    /// `resolve::column_width::resolve`, which guarantees `ranges` is
    /// already sorted by `min` and mutually non-overlapping — this method
    /// trusts that precondition rather than re-validating it (Issue #39).
    pub(crate) fn set_col_widths(&mut self, ranges: Vec<ColWidthRange>, default: Option<f64>) {
        self.col_widths = ranges;
        self.default_col_width = default;
    }

    /// The width of column `col`, or `None` if it falls in no `<col>`
    /// range and no `<sheetFormatPr defaultColWidth>` was set. Binary
    /// search over `col_widths` (sorted, non-overlapping by construction —
    /// see `set_col_widths`): find the last range with `min <= col`, then
    /// check whether it actually reaches `col` — O(log R) for R ranges,
    /// never O(R) regardless of how a file arranges them (Issue #39;
    /// same class of concern as `resolve_origin`'s Issue #43 fix).
    ///
    /// Returns `None` rather than a guessed fallback (e.g. Excel's common
    /// "Calibri 11" default of ~8.43 characters) when nothing covers
    /// `col`: that default depends on the workbook's font metrics, which
    /// this library does not compute — a wrong guess would be worse than
    /// an explicit absence for a caller to fill in its own default.
    pub fn column_width(&self, col: u32) -> Option<f64> {
        let idx = self.col_widths.partition_point(|r| r.min <= col);
        if idx > 0 {
            let candidate = &self.col_widths[idx - 1];
            if col <= candidate.max {
                return Some(candidate.width);
            }
        }
        self.default_col_width
    }

    /// The raw column-width ranges, for JSON output as a sheet-level
    /// `columns` array rather than duplicated onto every cell (Issue #36
    /// review discussion: embedding a column-level value per cell would
    /// multiply output size by the number of populated cells in each
    /// column, undermining the sparse-output design this library exists
    /// for).
    pub fn col_width_ranges(&self) -> &[ColWidthRange] {
        &self.col_widths
    }

    pub fn default_col_width(&self) -> Option<f64> {
        self.default_col_width
    }

    /// Registers this sheet's images (Issue #65). Called once by
    /// `pipeline::run` after resolving every `<xdr:pic>`'s `r:embed`/
    /// hyperlink relationship against `drawingN.xml.rels`.
    pub(crate) fn set_images(&mut self, images: Vec<Image>) {
        self.images = images;
    }

    /// The sheet's images, for JSON output as a sheet-level `images` array —
    /// the same sparse-output rationale as `col_width_ranges`.
    pub fn images(&self) -> &[Image] {
        &self.images
    }

    /// Backfills a blank placeholder cell at `r` if none exists yet
    /// (Issue #95) — same reasoning as `insert_merge`'s origin backfill.
    fn backfill_blank_cell(&mut self, r: CellRef) {
        if !self.cells.contains_key(&r) {
            self.insert_cell(
                r,
                Cell {
                    value: None,
                    style: None,
                },
            );
        }
    }

    /// Registers every validated hyperlink range at once (Issue #95;
    /// called by `resolve::hyperlink::resolve` after it validates the
    /// batch for reversed start/end and mutual overlap). Backfills each
    /// range's own origin cell, then resolves every cell key already in
    /// `cells` (this backfill included) to its covering range's
    /// `Hyperlink` in a single sweep-line pass — same Start/End/Query
    /// shape as `finalize_merges`, O((C + H) log (C + H)) for C cells and
    /// H ranges, never O(C * H).
    ///
    /// Unlike `finalize_merges` (which drops every non-origin key), a
    /// match here inserts into `hyperlinks` keyed by the *query*
    /// coordinate itself: `<hyperlink ref="A1:C3">` is not necessarily a
    /// merge, so every covered cell must carry the hyperlink
    /// independently in JSON output rather than collapsing into the
    /// range's origin. The origin cell picks up its own entry through the
    /// same pass, since `backfill_blank_cell` above already guaranteed it
    /// exists in `cells` before the sweep runs.
    ///
    /// Only a range's own origin is backfilled — a cell elsewhere in the
    /// range with no value/style of its own stays un-materialized and
    /// therefore invisible to `iter_cells`/JSON output, even though Excel
    /// would still show it as clickable. Deliberate, accepted limitation
    /// (see docs/design/resolve/hyperlink.en.md's Open Questions):
    /// backfilling every covered cell would reopen the same O(row_span *
    /// col_span) amplification `insert_merge`'s `merge_aliases` removal
    /// closed off for merges, since nothing bounds how large a
    /// legitimately-sized range can be.
    ///
    /// Preconditions (enforced by the caller, `resolve::hyperlink::
    /// resolve`, not here): `ranges` contains no two mutually-overlapping
    /// ranges, and every range has `start <= end`. Both are required for
    /// the sweep's column-sorted-active-set shortcut to be sound — the
    /// same precondition `finalize_merges` relies on from
    /// `resolve::merge`'s own validation.
    pub(crate) fn finalize_hyperlinks(&mut self, ranges: Vec<HyperlinkRange>) {
        if ranges.is_empty() {
            return;
        }
        for range in &ranges {
            self.backfill_blank_cell(range.start);
        }

        enum SweepEvent {
            /// Fired at `ranges[i].start.row`.
            Start(usize),
            /// Fired at `ranges[i].end.row + 1`.
            End(usize),
            /// Fired at the row of a cell coordinate to resolve.
            Query(CellRef),
        }

        let mut events: Vec<(u32, u8, SweepEvent)> =
            Vec::with_capacity(ranges.len() * 2 + self.cells.len());
        for (i, range) in ranges.iter().enumerate() {
            events.push((range.start.row, 0, SweepEvent::Start(i)));
            events.push((range.end.row + 1, 0, SweepEvent::End(i)));
        }
        for &coord in self.cells.keys() {
            events.push((coord.row, 2, SweepEvent::Query(coord)));
        }
        // Tie-break: at the same row, End/Start (rank 0/1 below) must be
        // applied before any Query at that row sees the active set.
        events.sort_by_key(|(row, kind_rank, event)| {
            let start_end_rank = match event {
                SweepEvent::End(_) => 0,
                SweepEvent::Start(_) => 1,
                SweepEvent::Query(_) => *kind_rank,
            };
            (*row, start_end_rank)
        });

        // Ranges active at the current sweep row, holding each range's
        // index into `ranges`, sorted by that range's `start.col`.
        // Disjoint column spans are guaranteed by the caller's overlap
        // validation, so at most one active entry can ever contain a
        // given query column.
        let mut active: Vec<usize> = Vec::new();

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
                    debug_assert_eq!(active.get(pos), Some(i));
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
    /// Mirrors `merged_region_at`'s convention exactly (no merge-origin
    /// resolution here — `json.rs` only ever calls this with coordinates
    /// already yielded by `iter_cells`, which are origins by
    /// construction); a caller starting from an arbitrary/virtual
    /// coordinate should resolve it via `get`/`iter_cells` first.
    pub fn hyperlink_at(&self, origin: CellRef) -> Option<&Hyperlink> {
        self.hyperlinks.get(&origin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(row: u32, col: u32) -> CellRef {
        CellRef { row, col }
    }

    #[test]
    fn get_on_blank_cell_returns_none() {
        let sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        assert!(sheet.get(r(1, 1)).is_none());
    }

    #[test]
    fn get_on_virtual_coordinate_returns_origin_cell() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            r(1, 1),
            Cell {
                value: Some(crate::model::CellValue::Boolean(true)),
                style: None,
            },
        );
        sheet.insert_merge(MergedRegion {
            start: r(1, 1),
            end: r(3, 3),
        });

        let origin = sheet.get(r(1, 1)).cloned();
        let virtual_cell = sheet.get(r(2, 2)).cloned();
        assert_eq!(origin, virtual_cell);
        assert!(virtual_cell.is_some());
    }

    #[test]
    fn get_mut_resolves_alias_and_allows_mutation() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            r(1, 1),
            Cell {
                value: Some(crate::model::CellValue::Boolean(false)),
                style: None,
            },
        );
        sheet.insert_merge(MergedRegion {
            start: r(1, 1),
            end: r(2, 2),
        });

        // Mutating through a virtual coordinate rewrites the origin cell.
        sheet.get_mut(r(2, 2)).unwrap().value = Some(crate::model::CellValue::Boolean(true));
        assert_eq!(
            sheet.get(r(1, 1)).unwrap().value,
            Some(crate::model::CellValue::Boolean(true))
        );

        assert!(sheet.get_mut(r(9, 9)).is_none());
    }

    #[test]
    fn merged_region_span() {
        let one_by_one = MergedRegion {
            start: r(1, 1),
            end: r(1, 1),
        };
        assert_eq!(one_by_one.row_span(), 1);
        assert_eq!(one_by_one.col_span(), 1);

        let large = MergedRegion {
            start: r(2, 3),
            end: r(10, 20),
        };
        assert_eq!(large.row_span(), 9);
        assert_eq!(large.col_span(), 18);
    }

    #[test]
    fn merged_region_at_lookup() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let region = MergedRegion {
            start: r(1, 1),
            end: r(2, 2),
        };
        sheet.insert_merge(region);
        assert_eq!(sheet.merged_region_at(r(1, 1)), Some(&region));
        assert_eq!(sheet.merged_region_at(r(2, 2)), None);
    }

    #[test]
    fn iter_cells_yields_row_major_then_column_major_order_regardless_of_insertion_order() {
        // Issue #87: json.rs's `cells` array feeds `iter_cells` directly,
        // so its order IS the JSON output order — this must be
        // deterministic (row ascending, then column ascending within a
        // row) for a textual diff of two snapshots of the same unchanged
        // file to stay quiet, independent of the order `<c>` elements
        // happened to stream in from the worksheet XML.
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let blank = || Cell {
            value: None,
            style: None,
        };
        // Deliberately scrambled insertion order — neither ascending nor
        // descending — so a coincidental match with insertion order can't
        // hide a bug.
        for (row, col) in [(3, 2), (1, 5), (2, 1), (1, 1), (3, 1), (2, 3), (1, 3)] {
            sheet.insert_cell(r(row, col), blank());
        }

        let coords: Vec<CellRef> = sheet.iter_cells().map(|(coord, _)| coord).collect();
        assert_eq!(
            coords,
            vec![
                r(1, 1),
                r(1, 3),
                r(1, 5),
                r(2, 1),
                r(2, 3),
                r(3, 1),
                r(3, 2),
            ]
        );
    }

    #[test]
    fn iter_cells_only_yields_origin_cells() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            r(1, 1),
            Cell {
                value: None,
                style: None,
            },
        );
        sheet.insert_merge(MergedRegion {
            start: r(1, 1),
            end: r(2, 2),
        });
        sheet.finalize_merges();

        let coords: Vec<CellRef> = sheet.iter_cells().map(|(coord, _)| coord).collect();
        assert_eq!(coords, vec![r(1, 1)]);
    }

    #[test]
    fn iter_cells_excludes_cells_pre_inserted_at_alias_coordinates() {
        // parse/worksheet.rs streams a `<c>` element for every cell it
        // encounters, including ones inside a merged range that later turn
        // out not to be the origin (e.g. border-only styling on B2 within
        // A1:B2). insert_merge must not let a pre-existing `cells` entry at
        // an alias coordinate leak into iter_cells.
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            r(1, 1),
            Cell {
                value: Some(crate::model::CellValue::Boolean(true)),
                style: None,
            },
        );
        sheet.insert_cell(
            r(2, 2),
            Cell {
                value: None,
                style: None,
            },
        );
        sheet.insert_merge(MergedRegion {
            start: r(1, 1),
            end: r(2, 2),
        });
        sheet.finalize_merges();

        let coords: Vec<CellRef> = sheet.iter_cells().map(|(coord, _)| coord).collect();
        assert_eq!(coords, vec![r(1, 1)]);
    }

    #[test]
    fn insert_cell_updates_max_row_col() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            r(5, 2),
            Cell {
                value: None,
                style: None,
            },
        );
        assert_eq!(sheet.max_row, 5);
        assert_eq!(sheet.max_col, 2);

        sheet.insert_cell(
            r(3, 9),
            Cell {
                value: None,
                style: None,
            },
        );
        assert_eq!(sheet.max_row, 5);
        assert_eq!(sheet.max_col, 9);
    }

    #[test]
    fn insert_merge_backfills_blank_origin_cell() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_merge(MergedRegion {
            start: r(1, 1),
            end: r(2, 2),
        });

        let origin_cell = sheet.get(r(1, 1));
        assert_eq!(
            origin_cell,
            Some(&Cell {
                value: None,
                style: None,
            })
        );
        assert_eq!(
            sheet.merged_region_at(r(1, 1)),
            Some(&MergedRegion {
                start: r(1, 1),
                end: r(2, 2),
            })
        );
        sheet.finalize_merges();
        let coords: Vec<CellRef> = sheet.iter_cells().map(|(coord, _)| coord).collect();
        assert_eq!(coords, vec![r(1, 1)]);
    }

    #[test]
    fn insert_merge_expands_used_range_from_only_a1_data() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            r(1, 1),
            Cell {
                value: Some(crate::model::CellValue::Boolean(true)),
                style: None,
            },
        );
        assert_eq!(sheet.max_row, 1);
        assert_eq!(sheet.max_col, 1);

        sheet.insert_merge(MergedRegion {
            start: r(1, 1),
            end: r(3, 3),
        });
        assert_eq!(sheet.max_row, 3);
        assert_eq!(sheet.max_col, 3);
    }

    #[test]
    fn insert_merge_on_huge_region_does_not_hang() {
        // Regression test: a full-sheet merge (Excel's actual maximum
        // dimensions, ~17 billion cells) must register in roughly constant
        // time, not time proportional to its area. An earlier
        // implementation populated a `HashMap<CellRef, CellRef>` alias
        // entry for every cell in the region inside `insert_merge`, which
        // hung in practice on a range this size.
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let huge = MergedRegion {
            start: r(1, 1),
            end: r(1_048_576, 16_384),
        };
        sheet.insert_merge(huge);

        assert_eq!(sheet.get(r(1, 1)), sheet.get(r(500_000, 8_000)));
        assert_eq!(sheet.merged_region_at(r(1, 1)), Some(&huge));
        assert_eq!(sheet.max_row, 1_048_576);
        assert_eq!(sheet.max_col, 16_384);
    }

    #[test]
    fn get_outside_any_merged_region_is_unaffected_by_other_regions() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            r(10, 10),
            Cell {
                value: Some(crate::model::CellValue::Boolean(true)),
                style: None,
            },
        );
        sheet.insert_merge(MergedRegion {
            start: r(1, 1),
            end: r(2, 2),
        });

        // r(10, 10) falls inside no merged region, so it must resolve to
        // itself rather than being swept into the unrelated A1:B2 region.
        assert_eq!(
            sheet.get(r(10, 10)),
            Some(&Cell {
                value: Some(crate::model::CellValue::Boolean(true)),
                style: None,
            })
        );
    }

    #[test]
    fn get_inside_bounding_box_but_outside_any_region_resolves_to_itself() {
        // Two merges with a gap between them: their combined merge_bounds
        // spans rows 1-10, but row 5 (inside the bounds, outside both
        // regions) must still resolve to itself rather than being
        // incorrectly swept into a region by the O(1) bounds pre-check.
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            r(5, 1),
            Cell {
                value: Some(crate::model::CellValue::Boolean(true)),
                style: None,
            },
        );
        sheet.insert_merge(MergedRegion {
            start: r(1, 1),
            end: r(2, 1),
        });
        sheet.insert_merge(MergedRegion {
            start: r(9, 1),
            end: r(10, 1),
        });

        assert_eq!(
            sheet.get(r(5, 1)),
            Some(&Cell {
                value: Some(crate::model::CellValue::Boolean(true)),
                style: None,
            })
        );
    }

    #[test]
    fn finalize_merges_is_a_no_op_when_there_are_no_merges() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            r(1, 1),
            Cell {
                value: Some(crate::model::CellValue::Boolean(true)),
                style: None,
            },
        );
        sheet.finalize_merges();

        let coords: Vec<CellRef> = sheet.iter_cells().map(|(coord, _)| coord).collect();
        assert_eq!(coords, vec![r(1, 1)]);
    }

    #[test]
    fn finalize_merges_drops_non_origin_cells_but_keeps_origin_and_standalone_cells() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        // A1: this merge's origin (real data).
        sheet.insert_cell(
            r(1, 1),
            Cell {
                value: Some(crate::model::CellValue::Boolean(true)),
                style: None,
            },
        );
        // B2: a virtual coordinate inside A1:B2, pre-populated the same way
        // parse/worksheet.rs would for border-only styling.
        sheet.insert_cell(
            r(2, 2),
            Cell {
                value: None,
                style: None,
            },
        );
        // C1: unrelated to any merge.
        sheet.insert_cell(
            r(1, 3),
            Cell {
                value: Some(crate::model::CellValue::Boolean(false)),
                style: None,
            },
        );
        sheet.insert_merge(MergedRegion {
            start: r(1, 1),
            end: r(2, 2),
        });
        sheet.finalize_merges();

        let mut coords: Vec<CellRef> = sheet.iter_cells().map(|(coord, _)| coord).collect();
        coords.sort();
        assert_eq!(coords, vec![r(1, 1), r(1, 3)]);
    }

    #[test]
    fn finalize_merges_resolves_multiple_disjoint_row_bound_merges_correctly() {
        // Two merges sharing no row overlap, each spanning several
        // columns, plus data cells scattered both inside and outside
        // either range — exercises the sweep line's Start/End bookkeeping
        // across more than one active region.
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        for col in 1..=5 {
            sheet.insert_cell(
                r(3, col),
                Cell {
                    value: Some(crate::model::CellValue::Number(col as f64)),
                    style: None,
                },
            );
            sheet.insert_cell(
                r(8, col),
                Cell {
                    value: Some(crate::model::CellValue::Number(col as f64)),
                    style: None,
                },
            );
        }
        sheet.insert_cell(
            r(5, 1),
            Cell {
                value: Some(crate::model::CellValue::Boolean(true)),
                style: None,
            },
        );
        sheet.insert_merge(MergedRegion {
            start: r(3, 1),
            end: r(3, 5),
        });
        sheet.insert_merge(MergedRegion {
            start: r(8, 1),
            end: r(8, 5),
        });
        sheet.finalize_merges();

        let mut coords: Vec<CellRef> = sheet.iter_cells().map(|(coord, _)| coord).collect();
        coords.sort();
        assert_eq!(coords, vec![r(3, 1), r(5, 1), r(8, 1)]);
        assert_eq!(
            sheet.get(r(3, 4)),
            Some(&Cell {
                value: Some(crate::model::CellValue::Number(1.0)),
                style: None,
            })
        );
    }

    #[test]
    fn column_width_with_no_ranges_and_no_default_is_none() {
        let sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        assert_eq!(sheet.column_width(1), None);
    }

    #[test]
    fn column_width_falls_back_to_default_col_width_outside_every_range() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.set_col_widths(
            vec![ColWidthRange {
                min: 1,
                max: 3,
                width: 12.0,
            }],
            Some(8.43),
        );
        assert_eq!(sheet.column_width(2), Some(12.0));
        assert_eq!(sheet.column_width(10), Some(8.43));
    }

    #[test]
    fn column_width_binary_search_across_multiple_ranges() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.set_col_widths(
            vec![
                ColWidthRange {
                    min: 1,
                    max: 5,
                    width: 10.0,
                },
                ColWidthRange {
                    min: 10,
                    max: 20,
                    width: 20.0,
                },
                ColWidthRange {
                    min: 100,
                    max: 16_384,
                    width: 5.0,
                },
            ],
            None,
        );

        assert_eq!(sheet.column_width(1), Some(10.0));
        assert_eq!(sheet.column_width(5), Some(10.0));
        assert_eq!(sheet.column_width(7), None); // gap between ranges
        assert_eq!(sheet.column_width(10), Some(20.0));
        assert_eq!(sheet.column_width(20), Some(20.0));
        assert_eq!(sheet.column_width(21), None); // gap
        assert_eq!(sheet.column_width(100), Some(5.0));
        assert_eq!(sheet.column_width(16_384), Some(5.0));
    }

    #[test]
    fn col_width_ranges_exposes_the_raw_ranges_for_json_output() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let ranges = vec![ColWidthRange {
            min: 1,
            max: 16_384,
            width: 8.43,
        }];
        sheet.set_col_widths(ranges.clone(), None);
        assert_eq!(sheet.col_width_ranges(), ranges.as_slice());
        assert_eq!(sheet.default_col_width(), None);
    }
}
