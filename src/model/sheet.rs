//! `Sheet`: the sparse-matrix data model for a single worksheet, including
//! transparent merged-cell access.

use crate::model::cell::{Cell, CellRef};
use std::collections::HashMap;

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
    /// virtual cell coordinate -> origin cell coordinate. Built by
    /// `resolve/merge.rs`.
    merge_aliases: HashMap<CellRef, CellRef>,
    /// origin cell coordinate -> merged region. Keying by the origin cell
    /// allows O(1) lookup of row_span/col_span.
    merged_regions: HashMap<CellRef, MergedRegion>,
    /// The largest row/column number among inserted cells. Updated
    /// incrementally on each cell insertion; does not depend on the
    /// `<dimension>` element's value.
    pub max_row: u32,
    pub max_col: u32,
}

impl Sheet {
    /// Constructs a new, empty sheet.
    ///
    /// `#[allow(dead_code)]`: only exercised by tests until `pipeline.rs`
    /// (Issue #15) calls it to build sheets from `parse/workbook.rs`'s
    /// output.
    #[allow(dead_code)]
    pub(crate) fn new(name: String, visibility: SheetVisibility) -> Self {
        Self {
            name,
            visibility,
            cells: HashMap::new(),
            merge_aliases: HashMap::new(),
            merged_regions: HashMap::new(),
            max_row: 0,
            max_col: 0,
        }
    }

    /// Retrieves a cell, resolving the merged-cell alias if needed. Returns
    /// the same `Cell` whether passed the origin or a virtual coordinate.
    pub fn get(&self, r: CellRef) -> Option<&Cell> {
        let origin = self.merge_aliases.get(&r).copied().unwrap_or(r);
        self.cells.get(&origin)
    }

    /// Retrieves a mutable reference to a cell, resolving the merged-cell
    /// alias if needed. Used by `resolve/shared_strings.rs` and
    /// `resolve/style.rs` to rewrite a cell's value/style with resolved
    /// data.
    ///
    /// `#[allow(dead_code)]`: unused until those `resolve/` modules
    /// (Issue #15) call it.
    #[allow(dead_code)]
    pub(crate) fn get_mut(&mut self, r: CellRef) -> Option<&mut Cell> {
        let origin = self.merge_aliases.get(&r).copied().unwrap_or(r);
        self.cells.get_mut(&origin)
    }

    /// Inserts a cell while updating max_row/max_col at the same time.
    /// Writes to `cells` only ever go through this method, structurally
    /// preventing the dimension fields from going out of sync.
    ///
    /// `#[allow(dead_code)]`: only exercised by tests until
    /// `parse/worksheet.rs` (Issue #15) calls it while streaming cells.
    #[allow(dead_code)]
    pub(crate) fn insert_cell(&mut self, r: CellRef, cell: Cell) {
        self.max_row = self.max_row.max(r.row);
        self.max_col = self.max_col.max(r.col);
        self.cells.insert(r, cell);
    }

    /// Registers a merged region: records every coordinate in the range
    /// (other than the origin) as an alias to the origin cell, and records
    /// the region itself keyed by the origin cell in `merged_regions`. If
    /// the origin cell does not yet exist in `cells` (a merged range with
    /// neither value nor formatting), a blank placeholder cell is inserted
    /// first, so `iter_cells` always picks up the origin cell. Calling this
    /// again for the same origin overwrites the previous region
    /// (last-write-wins).
    ///
    /// `#[allow(dead_code)]`: only exercised by tests until
    /// `resolve/merge.rs` (Issue #15) calls it.
    #[allow(dead_code)]
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
        for row in region.start.row..=region.end.row {
            for col in region.start.col..=region.end.col {
                let r = CellRef { row, col };
                if r != region.start {
                    self.merge_aliases.insert(r, region.start);
                }
            }
        }
        self.merged_regions.insert(region.start, region);
        self.max_row = self.max_row.max(region.end.row);
        self.max_col = self.max_col.max(region.end.col);
    }

    /// Retrieves, in O(1), the merged region an origin cell belongs to (used
    /// by `json.rs` to compute row_span/col_span).
    pub fn merged_region_at(&self, origin: CellRef) -> Option<&MergedRegion> {
        self.merged_regions.get(&origin)
    }

    /// An iterator over origin cells only (for JSON generation). A
    /// coordinate that is a merge alias is excluded even if `cells` holds an
    /// entry for it: `parse/worksheet.rs` inserts a `Cell` for every `<c>`
    /// element it streams, including ones inside a merged range that later
    /// turn out not to be the origin (e.g. a virtual cell carrying only
    /// border styling), so `cells` cannot be assumed to hold origin cells
    /// exclusively (PR #20 review).
    pub fn iter_cells(&self) -> impl Iterator<Item = (CellRef, &Cell)> {
        self.cells
            .iter()
            .filter(|(r, _)| !self.merge_aliases.contains_key(r))
            .map(|(&r, c)| (r, c))
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
}
