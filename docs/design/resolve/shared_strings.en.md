# `resolve/shared_strings.rs` Design Doc

*[日本語](shared_strings.md)*

Design doc for `src/resolve/shared_strings.rs`. This handles the "shared string (SST) index resolution" part of Phase 4 as defined by [architecture.md](../architecture.en.md). It resolves the index into the shared string table held by `t="s"` cells into the actual string (`model::CellValue::Text`).

## Responsibility / Scope

- Takes the "pending list of cells referencing shared-string indices" recorded by Phase 3 (`parse/worksheet.rs`), looks each up in the `SharedStringTable` (built by `parse/shared_strings.rs`, not yet designed) to resolve the actual string, and writes it back to the corresponding cell in `Sheet`
- Returns `Error::SharedStringIndexOutOfBounds` if an index is out of the table's range
- **Not responsible for**: XML parsing of `sharedStrings.xml` and building the `SharedStringTable` itself (`parse/shared_strings.rs`, not yet designed); resolving inline strings (`t="inlineStr"`) or formula strings (`t="str"`) — as [model/cell.md](../model/cell.en.md) states, these are also ultimately unified into `CellValue::Text`, but unlike `t="s"` they require no lookup table, so `parse/worksheet.rs` can insert them directly as `CellValue::Text` into `Sheet` while streaming. This file handles only the deferred resolution of `t="s"`

## Key Types / Functions (draft)

```rust
use crate::error::Error;
use crate::model::cell::CellValue;
use crate::model::sheet::Sheet;
use crate::parse::shared_strings::SharedStringTable;
// PendingSharedString is Phase 3's own output data, so parse/worksheet.rs
// defines it (reflects the PR #9 review — see Dependencies).
use crate::parse::worksheet::PendingSharedString;

/// For each entry in `pending`, looks up the actual string in `table` and
/// writes it back into the corresponding cell in `sheet` as `CellValue::Text`.
pub(crate) fn resolve(
    sheet: &mut Sheet,
    pending: &[PendingSharedString],
    table: &SharedStringTable,
) -> Result<(), Error> {
    for entry in pending {
        let text = table.get(entry.index).ok_or(Error::SharedStringIndexOutOfBounds {
            index: entry.index,
            len: table.len(),
        })?;
        // Assumes Phase 3 has already inserted a cell at the same cell_ref
        // (see resolve/mod.rs's calling precondition).
        let cell = sheet
            .get_mut(entry.cell_ref)
            .expect("pending shared string references a cell not inserted by parse/worksheet.rs");
        cell.value = Some(CellValue::Text(text.clone()));
    }
    Ok(())
}
```

## Dependencies

- Depends on: [`model/sheet.rs`](../model/sheet.en.md) (`Sheet::get_mut`), [`model/cell.rs`](../model/cell.en.md) (`CellValue::Text`), [`error.rs`](../error.en.md), [`parse::shared_strings::SharedStringTable`](../parse/shared_strings.en.md), [`parse::worksheet::PendingSharedString`](../parse/worksheet.en.md)
- Depended on by: [`resolve/mod.rs`](mod.en.md) (called from `resolve_sheet`)

Using `expect` rather than propagating an `Option`/`Result` from `get_mut` might appear to contradict [model/sheet.md](../model/sheet.en.md)'s policy that "`get`/`get_mut` represent a missing cell as `Option`, treating it as a normal case." The difference is that here, "the cell doesn't exist" does not originate from user input (the XLSX file) — it can only occur if `parse/worksheet.rs` recorded a `PendingSharedString` but forgot to call the matching `insert_cell`, i.e. an internal crate programming error (see Error Handling Policy for details).

## Error Handling Policy

- If `table.get(index)` returns `None` (index out of the table's range), returns `Error::SharedStringIndexOutOfBounds`. Since this can stem from untrusted external input (a malformed `.xlsx`), it is propagated as a `Result` rather than `panic`.
- If `sheet.get_mut(entry.cell_ref)` returns `None` (the cell a `PendingSharedString` refers to does not exist in `Sheet`), this does not indicate malformed external input but an implementation defect in `parse/worksheet.rs` (the invariant that a `PendingSharedString` record and its matching `insert_cell` call must always be paired when a `t="s"` cell is detected was violated). This is therefore a `panic` via `expect`, not a `Result`. This invariant will be documented formally when `parse/worksheet.rs` is designed (see Open Question 2).

## Testing Strategy

- Verify that a `PendingSharedString` with a valid index resolves correctly to `CellValue::Text` holding the corresponding string from `SharedStringTable`
- Verify that an out-of-range index (equal to `table.len()`, or far exceeding it) returns `Error::SharedStringIndexOutOfBounds` with the correct `index`/`len` values
- Verify that when multiple `PendingSharedString` entries referencing the same string are resolved, the resulting `CellValue::Text` values' `Arc<str>` are identical under `Arc::ptr_eq` (no duplicate allocation) — a wiring check against [model/cell.md](../model/cell.en.md)'s `Arc<str>` design policy
- Verify that an empty `pending` list results in a no-op `Ok(())`

## Open Questions

1. ~~Type and location of `SharedStringTable`~~ → **Resolved**: [`parse/shared_strings.rs`](../parse/shared_strings.en.md) defines it as a wrapper around `Vec<Arc<str>>`, exposing `get(index) -> Option<&Arc<str>>` and `len()`.
2. ~~Formalizing the invariant shared with `parse/worksheet.rs`~~ → **Resolved**: the contract "whenever a `t="s"` cell is detected, recording a `PendingSharedString` and calling `insert_cell` with an empty `Cell` (`value: None`) must always happen together" is recorded as the source of truth in [`parse/worksheet.rs`](../parse/worksheet.en.md)'s own documentation.
3. ~~Validity of the resolution timing for formula cells (`t="str"`) and inline strings (`t="inlineStr"`)~~ → **Confirmed**: [`parse/worksheet.rs`](../parse/worksheet.en.md)'s design settled on resolving these directly to `CellValue::Text` during the stream, as assumed. The actual cost of wrapping in `Arc<str>` is left to implementation-time profiling.
