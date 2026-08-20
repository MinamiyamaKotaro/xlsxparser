// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Phase 4: analysis and deferred resolution. I/O-independent — operates
//! purely on in-memory data (`model::Sheet` and the pending lists Phase 3
//! built), per `docs/design/architecture.en.md` design principle 2.

mod color;
mod column_width;
pub(crate) mod hyperlink;
mod merge;
mod shared_strings;
mod style;

pub use color::resolve_color;

use crate::error::Error;
use crate::model::{ColWidthRange, MergedRegion, Sheet, StyleSheet};
use crate::parse::{PendingSharedString, PendingStyle, SharedStringTable};

/// Runs Phase 4 resolution over one sheet's worth of unresolved data.
/// Intended to be called once per sheet by `pipeline.rs`.
///
/// Preconditions: `sheet` has already had all cells inserted by Phase 3
/// (`parse/worksheet.rs`). However, cells referencing shared strings are
/// still inserted with `value: None`, and cells referencing styles with
/// `style: None`.
///
/// Runs shared-string resolution, then style application, then column
/// width, then merge resolution; if any step fails, the remaining steps do
/// not run, so a partially-resolved sheet is never returned to the caller
/// (fail closed).
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_sheet(
    sheet: &mut Sheet,
    pending_shared_strings: &[PendingSharedString],
    shared_string_table: &SharedStringTable,
    pending_styles: &[PendingStyle],
    stylesheet: &StyleSheet,
    date1904: bool,
    col_width_ranges: Vec<ColWidthRange>,
    default_col_width: Option<f64>,
    merge_regions: Vec<MergedRegion>,
) -> Result<(), Error> {
    shared_strings::resolve(sheet, pending_shared_strings, shared_string_table)?;
    style::resolve(sheet, pending_styles, stylesheet, date1904)?;
    column_width::resolve(sheet, col_width_ranges, default_col_width)?;
    merge::resolve(sheet, merge_regions)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Cell, CellRef, CellValue, ResolvedStyle, SheetVisibility};
    use std::sync::Arc;

    #[test]
    fn resolves_shared_string_style_and_merge_together() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let sst_ref = CellRef { row: 1, col: 1 };
        let styled_ref = CellRef { row: 2, col: 1 };
        sheet.insert_cell(
            sst_ref,
            Cell {
                value: None,
                style: None,
            },
        );
        sheet.insert_cell(
            styled_ref,
            Cell {
                value: Some(CellValue::Number(1.0)),
                style: None,
            },
        );

        let table = crate::parse::parse_shared_strings(
            &b"<sst><si><t>hello</t></si></sst>"[..],
            "xl/sharedStrings.xml",
        )
        .unwrap();
        let mut stylesheet = StyleSheet::new();
        stylesheet.insert(
            0,
            Arc::new(ResolvedStyle {
                is_date_time: false,
                ..Default::default()
            }),
        );
        let merge_regions = vec![MergedRegion {
            start: CellRef { row: 3, col: 1 },
            end: CellRef { row: 3, col: 2 },
        }];

        resolve_sheet(
            &mut sheet,
            &[PendingSharedString {
                cell_ref: sst_ref,
                index: 0,
            }],
            &table,
            &[PendingStyle {
                cell_ref: styled_ref,
                style_id: 0,
            }],
            &stylesheet,
            false,
            vec![ColWidthRange {
                min: 1,
                max: 5,
                width: 12.0,
            }],
            None,
            merge_regions,
        )
        .unwrap();

        assert_eq!(
            sheet.get(sst_ref).unwrap().value,
            Some(CellValue::Text(Arc::from("hello")))
        );
        assert!(sheet.get(styled_ref).unwrap().style.is_some());
        assert_eq!(
            sheet.get(CellRef { row: 3, col: 2 }),
            sheet.get(CellRef { row: 3, col: 1 })
        );
        assert_eq!(sheet.column_width(1), Some(12.0));
    }

    #[test]
    fn shared_string_failure_skips_subsequent_steps() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let cell_ref = CellRef { row: 1, col: 1 };
        sheet.insert_cell(
            cell_ref,
            Cell {
                value: None,
                style: None,
            },
        );
        let empty_table =
            crate::parse::parse_shared_strings(&b"<sst></sst>"[..], "xl/sharedStrings.xml")
                .unwrap();
        let stylesheet = StyleSheet::new();
        // A malformed merge range that would itself fail validation, to
        // prove merge resolution never even runs after the earlier failure.
        let bad_merge_regions = vec![MergedRegion {
            start: CellRef { row: 3, col: 3 },
            end: CellRef { row: 1, col: 1 },
        }];

        let err = resolve_sheet(
            &mut sheet,
            &[PendingSharedString { cell_ref, index: 0 }],
            &empty_table,
            &[],
            &stylesheet,
            false,
            vec![],
            None,
            bad_merge_regions,
        )
        .unwrap_err();

        assert!(matches!(err, Error::SharedStringIndexOutOfBounds { .. }));
        assert!(sheet.merged_region_at(CellRef { row: 3, col: 3 }).is_none());
    }

    #[test]
    fn empty_pending_and_merge_lists_are_a_no_op() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let table = crate::parse::parse_shared_strings(&b"<sst></sst>"[..], "xl/sharedStrings.xml")
            .unwrap();
        let stylesheet = StyleSheet::new();
        resolve_sheet(
            &mut sheet,
            &[],
            &table,
            &[],
            &stylesheet,
            false,
            vec![],
            None,
            vec![],
        )
        .unwrap();
    }
}
