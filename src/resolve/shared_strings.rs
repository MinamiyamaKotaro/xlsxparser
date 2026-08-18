// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Phase 4: resolves `t="s"` cells' shared-string indices into the actual
//! string, writing it back into `Sheet`.

use crate::error::Error;
use crate::model::{CellValue, Sheet};
use crate::parse::{PendingSharedString, SharedStringTable};

/// For each entry in `pending`, looks up the actual string in `table` and
/// writes it back into the corresponding cell in `sheet` as
/// `CellValue::Text`.
pub(crate) fn resolve(
    sheet: &mut Sheet,
    pending: &[PendingSharedString],
    table: &SharedStringTable,
) -> Result<(), Error> {
    for entry in pending {
        let text = table
            .get(entry.index)
            .ok_or(Error::SharedStringIndexOutOfBounds {
                index: entry.index,
                len: table.len(),
            })?;
        // Assumes Phase 3 has already inserted a cell at the same cell_ref
        // (parse/worksheet.rs's calling contract).
        let cell = sheet
            .get_mut(entry.cell_ref)
            .expect("pending shared string references a cell not inserted by parse/worksheet.rs");
        cell.value = Some(CellValue::Text(text.clone()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CellRef, SheetVisibility};
    use std::sync::Arc;

    fn sheet_with_pending_cell(cell_ref: CellRef) -> Sheet {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            cell_ref,
            crate::model::Cell {
                value: None,
                style: None,
            },
        );
        sheet
    }

    fn table(strings: &[&str]) -> SharedStringTable {
        // parse_shared_strings is the only public constructor; build a
        // minimal sharedStrings.xml to exercise it rather than depending on
        // private fields.
        let mut xml = String::from("<sst>");
        for s in strings {
            xml.push_str(&format!("<si><t>{s}</t></si>"));
        }
        xml.push_str("</sst>");
        crate::parse::parse_shared_strings(xml.as_bytes(), "xl/sharedStrings.xml").unwrap()
    }

    #[test]
    fn resolves_valid_index_to_text() {
        let cell_ref = CellRef { row: 1, col: 1 };
        let mut sheet = sheet_with_pending_cell(cell_ref);
        let table = table(&["hello", "world"]);

        resolve(
            &mut sheet,
            &[PendingSharedString { cell_ref, index: 1 }],
            &table,
        )
        .unwrap();

        assert_eq!(
            sheet.get(cell_ref).unwrap().value,
            Some(CellValue::Text(Arc::from("world")))
        );
    }

    #[test]
    fn out_of_range_index_is_an_error() {
        let cell_ref = CellRef { row: 1, col: 1 };
        let mut sheet = sheet_with_pending_cell(cell_ref);
        let table = table(&["only"]);

        let err = resolve(
            &mut sheet,
            &[PendingSharedString { cell_ref, index: 1 }],
            &table,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            Error::SharedStringIndexOutOfBounds { index: 1, len: 1 }
        ));
    }

    #[test]
    fn duplicate_string_references_share_the_same_arc_allocation() {
        let ref_a = CellRef { row: 1, col: 1 };
        let ref_b = CellRef { row: 1, col: 2 };
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        sheet.insert_cell(
            ref_a,
            crate::model::Cell {
                value: None,
                style: None,
            },
        );
        sheet.insert_cell(
            ref_b,
            crate::model::Cell {
                value: None,
                style: None,
            },
        );
        let table = table(&["shared"]);

        resolve(
            &mut sheet,
            &[
                PendingSharedString {
                    cell_ref: ref_a,
                    index: 0,
                },
                PendingSharedString {
                    cell_ref: ref_b,
                    index: 0,
                },
            ],
            &table,
        )
        .unwrap();

        let (Some(CellValue::Text(a)), Some(CellValue::Text(b))) = (
            &sheet.get(ref_a).unwrap().value,
            &sheet.get(ref_b).unwrap().value,
        ) else {
            unreachable!()
        };
        assert!(Arc::ptr_eq(a, b));
    }

    #[test]
    fn empty_pending_list_is_a_no_op() {
        let mut sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        let table = table(&[]);
        resolve(&mut sheet, &[], &table).unwrap();
    }
}
