# `resolve/style.rs` Design Doc

*[日本語](style.md)*

Design doc for `src/resolve/style.rs`. This handles the "cell style application" part of Phase 4 as defined by [architecture.md](../architecture.en.md). It applies style definitions obtained from [`model/style.rs`](../model/style.en.md)'s `StyleSheet` to each cell, and also performs the `CellValue::Number` → `CellValue::DateTime` conversion when a numeric format (numFmt) represents a date/time.

## Responsibility / Scope

- Takes the "pending list of cells referencing a style ID" recorded by Phase 3 (`parse/worksheet.rs`), looks each up in [`model::style::StyleSheet`](../model/style.en.md) to obtain the resolved style (`ResolvedStyle`), and sets it on each cell's `style: Option<Arc<ResolvedStyle>>`
- When the applied style's numeric format (numFmt) is determined to be a date/time format, converts the target cell's `CellValue::Number` to `CellValue::DateTime`. For a value that cannot be converted (negative, `NaN`, `Infinity`, outside Excel's representable range, etc.), skips the conversion and continues processing with `CellValue::Number` left unchanged (a fallback — addresses PR #8 review feedback, resolving Open Question 3; see Error Handling Policy for details)
- Performs the actual serial-value-to-calendar decomposition in `serial_to_date_time` (Issue #40). `resolve()` takes `date1904: bool` (the `<workbookPr date1904="1"/>` flag [`parse/workbook.rs`](../parse/workbook.en.md) reads) as a parameter, selecting between the 1900 and 1904 date-system epochs
- Returns `Error::InvalidStyleId` if a style ID is out of the style definitions' range
- **Not responsible for**: XML parsing of `styles.xml` and the logic that builds `ResolvedStyle` itself (`parse/styles.rs` — this file assumes it receives an already-built `StyleSheet`); the type definitions of `ResolvedStyle` / `StyleSheet` / `StyleId` themselves (moved to [`model/style.rs`](../model/style.en.md) — addresses PR #8 review feedback, resolving Open Question 1); the concrete implementation of the numFmt code rules that determine whether a format is a date/time format (this file assumes `ResolvedStyle` already holds the determination result — see Open Question 2); reading the `date1904` flag itself ([`parse/workbook.rs`](../parse/workbook.en.md) — this file only consumes the value it was handed)

## Key Types / Functions (draft)

```rust
use crate::error::Error;
use crate::model::cell::{CellValue, DateTimeValue};
use crate::model::sheet::Sheet;
use crate::model::style::{ResolvedStyle, StyleSheet};
// PendingStyle is Phase 3's own output data, so parse/worksheet.rs defines
// it (reflects the PR #9 review — see Dependencies).
use crate::parse::worksheet::PendingStyle;

/// For each entry in `pending`, looks up `ResolvedStyle` in `stylesheet` and
/// sets it on the corresponding cell in `sheet`. Also converts
/// `CellValue::Number` to `CellValue::DateTime` when an `is_date_time`
/// format is applied. `date1904` (`<workbookPr date1904="1"/>`, read once in
/// Phase 1) selects which of Excel's two serial-value epochs to use — see
/// `serial_to_date_time` (Issue #40).
pub(crate) fn resolve(
    sheet: &mut Sheet,
    pending: &[PendingStyle],
    stylesheet: &StyleSheet,
    date1904: bool,
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
                // Leave CellValue::Number unchanged when conversion isn't
                // possible (fallback — see Error Handling Policy).
                if let Some(dt) = serial_to_date_time(serial, date1904) {
                    cell.value = Some(CellValue::DateTime(dt));
                }
            }
        }
        cell.style = Some(resolved);
    }
    Ok(())
}

/// Converts an Excel serial value into a `DateTimeValue`, using the 1900
/// date system (epoch equivalent to 1899-12-30) or the 1904 system (epoch
/// 1904-01-01) depending on `date1904`. Returns `None` (not an error — see
/// Error Handling Policy) for values that cannot be converted: negative,
/// `NaN`, `Infinity`, or outside Excel's representable range (up to roughly
/// December 31, 9999), etc.
///
/// **1900 leap-year bug**: for the 1900 system, plain epoch-offset
/// arithmetic alone is *not* sufficient — verified directly (working
/// backward from concrete known dates) that ordinary proleptic-Gregorian
/// arithmetic lands one day early for every serial in `1..60` (e.g. serial
/// 1 comes out as 1899-12-31 rather than Excel's own 1900-01-01), and
/// cannot represent serial 60 at all, since the real Gregorian calendar has
/// no "1900-02-29" (1900 is not actually a leap year — Microsoft KB214326
/// documents Excel's own fictitious reporting of it). The fix: shift
/// serials 1-59 forward by one day before applying the offset (the same
/// technique openpyxl and most other Excel-compatible readers use), and
/// hardcode serial 60 directly to the fictitious date Excel itself reports.
/// The date-part conversion itself uses Howard Hinnant's `civil_from_days`
/// algorithm (public domain, integer arithmetic only — no dependency on
/// `chrono` or any other date/time crate, per Issue #40's stated
/// performance requirement). The 1904 system has no leap-year bug (1904 is
/// a genuine leap year).
fn serial_to_date_time(serial: f64, date1904: bool) -> Option<DateTimeValue> {
    let _ = (serial, date1904);
    unimplemented!()
}
```

## Dependencies

- Depends on: [`model/sheet.rs`](../model/sheet.en.md) (`Sheet::get_mut`), [`model/cell.rs`](../model/cell.en.md) (`CellValue`, `DateTimeValue`), [`model/style.rs`](../model/style.en.md) (`ResolvedStyle`, `StyleSheet` — moved out of this file, addressing PR #8 review feedback), [`error.rs`](../error.en.md), [`parse::worksheet::PendingStyle`](../parse/worksheet.en.md)
- Depended on by: [`resolve/mod.rs`](mod.en.md) (called from `resolve_sheet`). `date1904` is read once in Phase 1 by [`pipeline.rs`](../pipeline.en.md) from [`parse/workbook.rs`](../parse/workbook.en.md) and passed straight through to the `resolve_sheet` call, never stored on `model::Workbook` itself — the same "phase-transient value" treatment [model/style.md](../model/style.en.md)'s `StyleSheet` gets

Defining `StyleSheet` / `ResolvedStyle` / `StyleId` in [`model/style.rs`](../model/style.en.md) rather than in `resolve/style.rs` itself means `parse/styles.rs` (not yet designed — the entity that builds `StyleSheet`) and `resolve/style.rs` (the entity that applies it) both depend only on `model/` and never know about each other directly (addresses PR #8 review feedback — see [model/style.md](../model/style.en.md) for details).

As with [`resolve/shared_strings.rs`](shared_strings.en.md), a `None` returned from `sheet.get_mut` is treated as an invariant violation on the `parse/worksheet.rs` side (an internal crate programming error) and handled with `expect` (see Error Handling Policy for details).

Why the `CellValue::Number` → `CellValue::DateTime` conversion happens inside the same function as style application: without looking at the numFmt (i.e. style information), there is no way to tell whether a given cell's `f64` is a plain number or a date serial value, so as [model/cell.md](../model/cell.en.md) already states, this conversion is this file's (`resolve/style.rs`'s) responsibility. Although [`resolve/shared_strings.rs`](shared_strings.en.md) is assumed to run before this step ([resolve/mod.md](mod.en.md)'s calling order), `CellValue::Text` values are never converted even when `is_date_time` is set (they are naturally excluded by the `if let Some(CellValue::Number(..))` pattern match), so swapping the ordering does not affect the correctness of this step itself.

## Error Handling Policy

- If `stylesheet.get(style_id)` returns `None` (an out-of-range `cellXfs` index), returns `Error::InvalidStyleId`. Since this can stem from untrusted external input, it does not `panic`.
- If `sheet.get_mut(entry.cell_ref)` returns `None`, this is handled with `expect` for the same reason as in [shared_strings.md Error Handling Policy](shared_strings.en.md) (an invariant violation on the `parse/worksheet.rs` side).
- **Date-conversion failure is not treated as an error; it falls back instead**: when `serial_to_date_time` returns `None` (a value that cannot be interpreted as a date — negative, `NaN`, `Infinity`, outside Excel's representable range, etc.), no `Error` is constructed or propagated to the caller. The target cell's `CellValue` is left as `Number(serial)`, and the overall resolution process continues as the normal-success path. Failing to parse the whole document over a single cell's unparseable date would make the library excessively fragile; since the cell's underlying value (which is a valid number) is not lost, this also leaves room for a downstream consumer to reinterpret it using the numFmt on its own (addresses PR #8 review feedback, resolving Open Question 3). Note that this policy differs from the "reject the whole batch, fail closed" policy adopted by [merge.md](merge.en.md) and [container/sanitize.md](../container/sanitize.en.md) for invalid input. The distinction: the latter deal with security threats (Zip Bomb/Slip) or structural inconsistencies (overlapping merged ranges), whereas this case is a soft failure to interpret a single cell's value, one that does not compromise the document's overall integrity.

## Testing Strategy

- Verify that a `PendingStyle` with a valid style ID correctly sets `Cell.style` to the corresponding `ResolvedStyle` from `StyleSheet`
- Verify that a nonexistent style ID (out of `StyleSheet`'s range) returns `Error::InvalidStyleId`
- Verify that applying an `is_date_time: true` style to a cell holding `CellValue::Number` converts it to `CellValue::DateTime` when the value is convertible
- **Verify that when an `is_date_time: true` style is applied to a cell holding `CellValue::Number` but the value is not convertible (negative, `NaN`, `Infinity`, etc.), no `Err` is returned and `CellValue::Number` is left unchanged** (a regression test for the fallback behavior added per the PR #8 review)
- Verify that applying an `is_date_time: true` style to a cell holding a non-numeric value (`CellValue::Text` / `CellValue::Boolean`, etc.) leaves `value` unconverted (a regression-test angle to guard against mis-conversion)
- Verify that applying a normal `is_date_time: false` style leaves `value` unconverted and only sets `style`
- Verify that applying the same `ResolvedStyle` to multiple cells results in `Cell.style` `Arc`s that are identical under `Arc::ptr_eq` (no duplicate allocation) — a wiring check against [model/cell.md](../model/cell.en.md)'s `Arc` design policy
- Verify that an empty `pending` list results in a no-op `Ok(())`
- **Verify serial 1 resolves to 1900-01-01 in the 1900 system** — a regression test for the naive "epoch offset alone" arithmetic, which would otherwise land on 1899-12-31 (Issue #40)
- **Verify serials 59 and 61 resolve to 1900-02-28 and 1900-03-01 respectively, and serial 60 resolves to the fictitious 1900-02-29 (Microsoft KB214326)** — boundary tests for the 1900 leap-year bug
- **Verify the 1904 system (`date1904: true`) has no leap-year bug at serial 60** — 1904 is a genuine leap year, so this serial should resolve to an ordinary, non-fictitious date
- **Verify a fractional serial decomposes correctly into hour/minute/second**, and that a fractional part that rounds to exactly 86,400 seconds (a full day) carries into the next day rather than producing an impossible `hour: 24` (a floating-point rounding boundary test)

## Open Questions

1. ~~Final location of `ResolvedStyle` / `StyleSheet` / `StyleId`~~ → **Resolved**: newly added [`model/style.rs`](../model/style.en.md) and defined them there. Having both `parse/styles.rs` (the builder) and `resolve/style.rs` (the applier) depend only on `model/` preserves independence between layers (addresses PR #8 review feedback).
2. ~~Where the date/time format determination logic lives~~ → **Resolved**: it lives on the [`parse/styles.rs`](../parse/styles.en.md) side (including OOXML numFmt determination). This file continues to receive `ResolvedStyle.is_date_time` as an already-determined value and holds none of the determination logic itself. The heuristic's precision remains open — see [parse/styles.md Open Question 2](../parse/styles.en.md).
3. ~~Implementation of `serial_to_date_time`~~ → **Resolved** (Issue #40): settled on a policy where values that cannot be converted return `None` rather than an `Error`, with the caller (this file's `resolve`) falling back to leaving `CellValue::Number` unchanged (addresses PR #8 review feedback), and the conversion formula itself (including the 1900 leap-year bug and the 1904 date system) is now implemented as described above. The design initially assumed the epoch offset alone would absorb the leap-year bug automatically; implementation-time verification (working backward from concrete known dates) showed this was wrong, and an explicit shift-plus-hardcode correction was needed instead — the same "measure and verify before committing" discipline this project has applied elsewhere (e.g. [Issue #43](https://github.com/MinamiyamaKotaro/xlsxparser/issues/43)'s performance investigation), here applied to date-conversion correctness instead.
4. **Concrete style elements such as font/fill/border**: the same point as [model/style.md Open Question 1](../model/style.en.md) (undecided). How far the requirements spec expects cell styling to be included in JSON output will be finalized alongside `json.rs`'s design, or as the requirements spec itself is elaborated.
