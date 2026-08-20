# `model/cell.rs` Design Doc

*[日本語](cell.md)*

Design doc for `src/model/cell.rs`. Per the [architecture.md](../architecture.en.md) policy for `model/` (pure Rust data structures with no dependency on XML parsing or resolution logic), this defines the most fundamental types representing a single cell's value and reference. `model/sheet.rs` and `model/workbook.rs` depend on the types defined here.

## Responsibility / Scope

- Defines the data for a single cell (`Cell`) and its value variants (`CellValue`)
- Defines `CellRef`, which converts between cell coordinates (row/column) and Excel's A1 notation (e.g. `"B12"`)
- **Not responsible for**: parsing from XML (`parse/worksheet.rs`), the actual resolution processing of shared strings/styles (`resolve/`), or merged-cell range determination logic (`resolve/merge.rs` — `Cell` itself has no knowledge of whether it belongs to a merged range)

## Key Types (draft)

```rust
use std::sync::Arc;

/// A resolved date/time value, decomposed into its calendar fields by
/// `resolve/style.rs::serial_to_date_time` from an Excel date serial number
/// (Issue #40; see Open Question 4). A plain `Copy` value type — no
/// sub-second precision, since Excel serial values (`f64`) can't reliably
/// represent it beyond float rounding noise, and no downstream use case has
/// asked for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DateTimeValue {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Cell coordinates. 1-based, matching Excel (A1 = row:1, col:1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CellRef {
    pub row: u32,
    pub col: u32,
}

impl CellRef {
    /// Builds from an "A1"-style string.
    pub fn from_a1(s: &str) -> Result<Self, crate::error::Error>;

    /// Converts to an "A1"-style string.
    pub fn to_a1(&self) -> String;
}

/// A cell's value. Has a variant corresponding to each OOXML `t` attribute (cell type).
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    /// The default when the t attribute is omitted. Non-date serial values live here.
    Number(f64),
    /// Converted from `Number` when `resolve/style.rs` determines, from the numFmt, that
    /// the value is a date/time. The concrete type is undecided (see Open Question 4).
    DateTime(DateTimeValue),
    /// A resolved string (shared string t="s" / inline str / str are all unified into this
    /// form once resolved). Uses `Arc<str>` to avoid duplicate allocations across cells that
    /// share the same string.
    Text(Arc<str>),
    Boolean(bool),
    /// t="e". Holds the error code string (e.g. "#DIV/0!") as-is.
    Error(String),
}

/// A single entry in the sparse matrix. Only cells that hold data or formatting exist in
/// `Sheet` (blank cells are not instantiated, per requirements spec 3.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// `Option` so that a cell with formatting only (no value) can be represented.
    pub value: Option<CellValue>,
    /// `None` represents the default (unset) style. `Arc` avoids duplicating identical
    /// styles across cells and decouples cell lifetime from the `StyleSheet` container's
    /// lifetime (see Dependencies).
    pub style: Option<Arc<ResolvedStyle>>,
}
```

`ResolvedStyle` is a type defined by [`model/style.rs`](style.en.md) (its location was settled per PR #8 review feedback); this file only assumes the type exists and uses it, without defining it. `DateTimeValue` is defined within this file (see Open Question 4).

## Dependencies

- Depends on: none (a leaf module with no dependency on any sibling module within `model/`)
- Depended on by: `model::Sheet` (uses `CellRef`, or an equivalent tuple, as the key of `BTreeMap<(u32, u32), Cell>` — see [model/sheet.en.md](sheet.en.md) for why `BTreeMap` rather than `HashMap`), `resolve/`, `json.rs`

`resolve/style.rs` clones the `Arc` for the relevant style out of [`model/style.rs`](style.en.md)'s `StyleSheet` (`HashMap<StyleId, Arc<ResolvedStyle>>`) and assigns it to each cell. Because each cell holds (a share of) the actual data's ownership via `Arc`, the `StyleSheet` container itself can be dropped once Phase 4 completes — satisfying both `pipeline.rs`'s policy of immediate disposal and the memory-efficiency requirement of avoiding value copies. The same relationship holds between `resolve/shared_strings.rs` and `Arc<str>`.

## Error Handling Policy

- `CellRef::from_a1` does not `panic` on invalid input (e.g. `"1A"`, empty string, column overflow) but returns a `Result`. Since all parser-originated input comes from an untrusted external file (XML), the common error type planned in `error.rs` is used. At implementation time, overflowing input such as `"A10000000000000"` must be reliably detected as an overflow during the `u32` parse and returned as `Err` (never `panic`).
- `CellValue::Error` merely passes through the OOXML error code as-is; the parser does not interpret or branch on it internally (that is the caller's responsibility).

## Testing Strategy

- Round-trip tests for `CellRef::from_a1` / `to_a1` (including boundary values such as `"A1"`, `"Z1"`, `"AA1"`, `"XFD1048576"` — Excel's maximum column/row)
- Verifying that invalid A1 strings (lowercase, mixed symbols, column-only, row-only, overflowing row number) return `Err`
- `PartialEq` comparison tests for each `CellValue` variant
- Verifying that `value: None` (a cell with formatting only) is correctly held and compared
- Verifying that `Arc::ptr_eq` holds true across multiple cells sharing the same style or string (i.e. the actual data is not duplicated)

## Open Questions

1. ~~Representation of the `style` field~~ → **Resolved**: adopt `Option<Arc<ResolvedStyle>>` (finalized following the [PR #5 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948235239)). `Arc` avoids duplicating the actual data while still allowing the `StyleSheet` container itself to be dropped once Phase 4 completes.
2. ~~Representation of shared strings~~ → **Resolved**: adopt `CellValue::Text(Arc<str>)`, for the same reason as above.
3. **Upper bound of rows/columns**: `u32` is sufficient for Excel's maximum column count (16,384 columns = XFD) and maximum row count (1,048,576 rows), but whether to treat `col` as a plain number or split it into a separate `ColumnRef` type in the future is undecided.
4. ~~Concrete type of `DateTimeValue`~~ → **Resolved** (Issue #40): a lightweight `Copy` struct with plain `year`/`month`/`day`/`hour`/`minute`/`second` fields, deliberately not depending on `chrono` or any other date/time crate — [`resolve/style.rs`](../resolve/style.en.md) decomposes an Excel serial value into it directly via a self-contained integer algorithm (Howard Hinnant's `civil_from_days`, public domain), keeping compile time and binary size unaffected. Excel's 1900 leap-year bug (serial 60 = the fictitious "1900-02-29") and the `<workbookPr date1904="1"/>` alternate epoch are both handled there — see [resolve/style.md](../resolve/style.en.md) for the conversion detail.
