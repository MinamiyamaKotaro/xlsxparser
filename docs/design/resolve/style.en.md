# `resolve/style.rs` Design Doc

*[日本語](style.md)*

Design doc for `src/resolve/style.rs`. This handles the "cell style application" part of Phase 4 as defined by [architecture.md](../architecture.en.md). It applies style definitions obtained from `styles.xml` (built by `parse/styles.rs`, not yet designed) to each cell, and also performs the `CellValue::Number` → `CellValue::DateTime` conversion when a numeric format (numFmt) represents a date/time. This resolves [model/cell.md](../model/cell.en.md) Open Question 3 (where `ResolvedStyle` is defined).

## Responsibility / Scope

- Takes the "pending list of cells referencing a style ID" recorded by Phase 3 (`parse/worksheet.rs`), looks each up in `StyleSheet` to obtain the resolved style (`ResolvedStyle`), and sets it on each cell's `style: Option<Arc<ResolvedStyle>>`
- When the applied style's numeric format (numFmt) is determined to be a date/time format, converts the target cell's `CellValue::Number` to `CellValue::DateTime` (this is the origin of the `DateTime` variant conversion mentioned in [model/cell.md](../model/cell.en.md))
- Returns `Error::InvalidStyleId` if a style ID is out of the style definitions' range
- **Not responsible for**: XML parsing of `styles.xml` and the logic that builds `ResolvedStyle` from `fonts`/`fills`/`borders`/`numFmts`/`cellXfs` itself (`parse/styles.rs`, not yet designed — this file assumes it receives an already-built `StyleSheet`); the concrete implementation of the numFmt code rules that determine whether a format is a date/time format (this file assumes `ResolvedStyle` already holds the determination result — see Open Question 2)

## Key Types / Functions (draft)

```rust
use std::collections::HashMap;
use std::sync::Arc;

use crate::error::Error;
use crate::model::cell::{CellValue, DateTimeValue};
use crate::model::sheet::{CellRef, Sheet};

/// The `cellXfs` index (style ID). Kept type-consistent with
/// [error.rs](../error.en.md)'s `Error::InvalidStyleId(u32)`.
pub type StyleId = u32;

/// Format information once a style ID has been resolved. Resolves
/// [model/cell.md Open Question 3](../model/cell.en.md); defined on this
/// file's side (the constituent fields may be revisited when `parse/styles.rs`
/// is designed — see Open Question 1).
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedStyle {
    /// Whether this format represents a date/time. `parse/styles.rs` is
    /// expected to interpret the `numFmts` code string (both built-in and
    /// custom) and store the determination result here ahead of time (see
    /// Open Question 2).
    pub is_date_time: bool,
    // Concrete fields for font/fill/border etc. will be finalized when parse/styles.rs is designed.
}

/// A table looking up `ResolvedStyle` by `cellXfs` index. Expected to be
/// built by `parse/styles.rs` (see Open Question 1).
pub type StyleSheet = HashMap<StyleId, Arc<ResolvedStyle>>;

/// A pending entry recorded when Phase 3 detects a cell with an `s` (style
/// index) attribute.
#[derive(Debug, Clone, Copy)]
pub struct PendingStyle {
    pub cell_ref: CellRef,
    pub style_id: StyleId,
}

/// For each entry in `pending`, looks up `ResolvedStyle` in `stylesheet` and
/// sets it on the corresponding cell in `sheet`. Also converts
/// `CellValue::Number` to `CellValue::DateTime` when an `is_date_time`
/// format is applied.
pub(crate) fn resolve(
    sheet: &mut Sheet,
    pending: &[PendingStyle],
    stylesheet: &StyleSheet,
) -> Result<(), Error> {
    for entry in pending {
        let resolved = stylesheet
            .get(&entry.style_id)
            .ok_or(Error::InvalidStyleId(entry.style_id))?
            .clone();
        let cell = sheet
            .get_mut(entry.cell_ref)
            .expect("pending style references a cell not inserted by parse/worksheet.rs");
        if resolved.is_date_time {
            if let Some(CellValue::Number(serial)) = cell.value {
                cell.value = Some(CellValue::DateTime(serial_to_date_time(serial)));
            }
        }
        cell.style = Some(resolved);
    }
    Ok(())
}

/// Converts an Excel serial value (including the 1900 leap-year bug epoch)
/// into a `DateTimeValue`. The concrete conversion formula and epoch
/// handling are undecided, tied to [model/cell.md Open Question 4](../model/cell.en.md).
fn serial_to_date_time(serial: f64) -> DateTimeValue {
    let _ = serial;
    unimplemented!()
}
```

## Dependencies

- Depends on: [`model/sheet.rs`](../model/sheet.en.md) (`Sheet::get_mut`, `CellRef`), [`model/cell.rs`](../model/cell.en.md) (`CellValue`, `DateTimeValue`), [`error.rs`](../error.en.md)
- Depended on by: [`resolve/mod.rs`](mod.en.md) (called from `resolve_sheet`). Once `parse/styles.rs` builds `StyleSheet` / `ResolvedStyle` / `StyleId` in the future, it is expected to depend on the types this file defines (see Open Question 1).

As with [`resolve/shared_strings.rs`](shared_strings.en.md), a `None` returned from `sheet.get_mut` is treated as an invariant violation on the `parse/worksheet.rs` side (an internal crate programming error) and handled with `expect` (see Error Handling Policy for details).

Why the `CellValue::Number` → `CellValue::DateTime` conversion happens inside the same function as style application: without looking at the numFmt (i.e. style information), there is no way to tell whether a given cell's `f64` is a plain number or a date serial value, so as [model/cell.md](../model/cell.en.md) already states, this conversion is this file's (`resolve/style.rs`'s) responsibility. Although [`resolve/shared_strings.rs`](shared_strings.en.md) is assumed to run before this step ([resolve/mod.md](mod.en.md)'s calling order), `CellValue::Text` values are never converted even when `is_date_time` is set (they are naturally excluded by the `if let Some(CellValue::Number(..))` pattern match), so swapping the ordering does not affect the correctness of this step itself.

## Error Handling Policy

- If `stylesheet.get(style_id)` returns `None` (an out-of-range `cellXfs` index), returns `Error::InvalidStyleId`. Since this can stem from untrusted external input, it does not `panic`.
- If `sheet.get_mut(entry.cell_ref)` returns `None`, this is handled with `expect` for the same reason as in [shared_strings.md Error Handling Policy](shared_strings.en.md) (an invariant violation on the `parse/worksheet.rs` side).
- Whether the date conversion `serial_to_date_time` can itself fail (e.g. a negative serial value, overflow) is undecided pending [model/cell.md Open Question 4](../model/cell.en.md)'s type finalization. Design proceeds for now assuming an implementation that never `panic`s (either returning `Result` or clamping boundary values — see Open Question 3).

## Testing Strategy

- Verify that a `PendingStyle` with a valid style ID correctly sets `Cell.style` to the corresponding `ResolvedStyle` from `StyleSheet`
- Verify that a nonexistent style ID (out of `StyleSheet`'s range) returns `Error::InvalidStyleId`
- Verify that applying an `is_date_time: true` style to a cell holding `CellValue::Number` converts it to `CellValue::DateTime`
- Verify that applying an `is_date_time: true` style to a cell holding a non-numeric value (`CellValue::Text` / `CellValue::Boolean`, etc.) leaves `value` unconverted (a regression-test angle to guard against mis-conversion)
- Verify that applying a normal `is_date_time: false` style leaves `value` unconverted and only sets `style`
- Verify that applying the same `ResolvedStyle` to multiple cells results in `Cell.style` `Arc`s that are identical under `Arc::ptr_eq` (no duplicate allocation) — a wiring check against [model/cell.md](../model/cell.en.md)'s `Arc` design policy
- Verify that an empty `pending` list results in a no-op `Ok(())`

## Open Questions

1. **Final location of `ResolvedStyle` / `StyleSheet` / `StyleId`**: Regarding the question [model/mod.md Open Question 1](../model/mod.en.md) left open — "undecided whether to place these in `model/` or on the `resolve/style.rs` side" — this file provisionally adopts defining them on the `resolve/style.rs` side. However, given that `parse/styles.rs` (not yet designed) will be the entity that actually constructs `ResolvedStyle`, it may turn out to be more appropriate to move the definitions to `parse/styles.rs`, or to a neutral location referenced by both modules (e.g. a new `model/style.rs`). This will be finalized when `parse/styles.rs` is designed.
2. **Where the date/time format determination logic lives**: This file assumes `ResolvedStyle.is_date_time` arrives as an already-determined value, but whether OOXML numFmt determination (range checks for built-in IDs 14–22 etc., pattern matching on custom format strings) happens on the `parse/styles.rs` side, or whether `resolve/style.rs` itself should carry the determination logic (with `ResolvedStyle` holding the raw format string and this file interpreting it), is undecided. Given architecture.md design principle 2 (`resolve/` is I/O-independent, but the determination logic itself is domain knowledge that is consistent either way), either placement is viable, so this will be finalized alongside `parse/styles.rs`'s design.
3. **Implementation of `serial_to_date_time`**: To be implemented once [model/cell.md Open Question 4](../model/cell.en.md) (the concrete type of `DateTimeValue`, handling of the 1900 leap-year bug) is resolved. Whether the function signature should return `Result` is also part of this open question.
4. **Concrete style elements such as font/fill/border**: `ResolvedStyle` currently only tentatively defines `is_date_time`; how far the requirements spec expects cell styling (font color, background color, borders, bold/italic, etc.) to be included in JSON output will be finalized alongside `json.rs`'s design, or as the requirements spec itself is elaborated.
