# `model/cell.rs` Design Doc

*[日本語](cell.md)*

Design doc for `src/model/cell.rs`. Per the [architecture.md](../architecture.en.md) policy for `model/` (pure Rust data structures with no dependency on XML parsing or resolution logic), this defines the most fundamental types representing a single cell's value and reference. `model/sheet.rs` and `model/workbook.rs` depend on the types defined here.

## Responsibility / Scope

- Defines the data for a single cell (`Cell`) and its value variants (`CellValue`)
- Defines `CellRef`, which converts between cell coordinates (row/column) and Excel's A1 notation (e.g. `"B12"`)
- **Not responsible for**: parsing from XML (`parse/worksheet.rs`), the actual resolution processing of shared strings/styles (`resolve/`), or merged-cell range determination logic (`resolve/merge.rs` — `Cell` itself has no knowledge of whether it belongs to a merged range)

## Key Types (draft)

```rust
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
    /// The default when the t attribute is omitted. Numbers and dates are included here as serial values.
    Number(f64),
    /// A resolved string (shared string t="s" / inline str / str are all unified into this form once resolved)
    Text(String),
    Boolean(bool),
    /// t="e". Holds the error code string (e.g. "#DIV/0!") as-is.
    Error(String),
}

/// A single entry in the sparse matrix. Only cells that hold data or formatting exist in
/// `Sheet` (blank cells are not instantiated, per requirements spec 3.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub value: CellValue,
    pub style: ResolvedStyle,
}
```

`ResolvedStyle` is a placeholder assuming another type within `model/` (planned to be defined either in `model/mod.rs` or on the `resolve/style.rs` side); this file only assumes the type exists, without defining it.

## Dependencies

- Depends on: none (a leaf module with no dependency on any sibling module within `model/`)
- Depended on by: `model::Sheet` (uses `CellRef`, or an equivalent tuple, as the key of `HashMap<(u32, u32), Cell>`), `resolve/`, `json.rs`

## Error Handling Policy

- `CellRef::from_a1` does not `panic` on invalid input (e.g. `"1A"`, empty string, column overflow) but returns a `Result`. Since all parser-originated input comes from an untrusted external file (XML), the common error type planned in `error.rs` is used.
- `CellValue::Error` merely passes through the OOXML error code as-is; the parser does not interpret or branch on it internally (that is the caller's responsibility).

## Testing Strategy

- Round-trip tests for `CellRef::from_a1` / `to_a1` (including boundary values such as `"A1"`, `"Z1"`, `"AA1"`, `"XFD1048576"` — Excel's maximum column/row)
- Verifying that invalid A1 strings (lowercase, mixed symbols, column-only, row-only) return `Err`
- `PartialEq` comparison tests for each `CellValue` variant

## Open Questions

1. **Representation of the `style` field**: As noted in the design memo for `pipeline.rs` ([architecture.md](../architecture.en.md#pipelinesrs)), it is undecided whether `Cell` holds resolved actual data (the `ResolvedStyle` value itself) or an index into the `StyleSheet` (e.g. `StyleId(u32)`). The former allows `StyleSheet` to be dropped after style resolution (more memory-efficient), but increases value copies when many cells share the same style. The latter avoids copies but requires extending `StyleSheet`'s lifetime until JSON generation completes.
2. **The same point applies to shared strings**: Whether `CellValue::Text` holds a resolved `String` or an index into the `SharedStringTable` carries the same trade-off as point 1 above. This file assumes the former (resolved) for now, to be finalized together with the `resolve/shared_strings.rs` design doc.
3. **Upper bound of rows/columns**: `u32` is sufficient for Excel's maximum column count (16,384 columns = XFD) and maximum row count (1,048,576 rows), but whether to treat `col` as a plain number or split it into a separate `ColumnRef` type in the future is undecided.
4. **Handling of dates**: Since OOXML represents dates as a serial value (`Number`) plus a numFmt in `styles.xml`, this file's type includes dates under `Number`, with the responsibility of determining/converting date-ness assumed to belong to `resolve/style.rs`. Needs confirmation whether this division of responsibility is acceptable.
