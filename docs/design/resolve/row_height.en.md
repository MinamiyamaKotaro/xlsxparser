# `resolve/row_height.rs` Design Document

*[日本語](row_height.md)*

Design document for `src/resolve/row_height.rs`, added under sister project exceldiff's Issue #51: Phase 4's "validate and register the row-height range batch." The row-axis counterpart of [`resolve/column_width.rs`](column_width.en.md). In exceldiff, this was added so `grid.rs`'s generated grid HTML reflects a real `.xlsx` file's actual row heights; this crate has no counterpart to `grid.rs`, so the purpose here is limited to exposing row-height data to callers through `Sheet::row_height` and `json.rs`'s `rows` array.

This file's structure deliberately mirrors `resolve/column_width.rs`, but the *shape of the source data* differs, which shifts where compression itself lives — see "Relationship to `resolve/column_width.rs`" below.

## Responsibilities / Scope

- Receives the row-height range list (`Vec<model::sheet::RowHeightRange>`) that Phase 3 (`parse/worksheet.rs`) already collected **pre-compressed** while streaming `<sheetData>`, plus `default_row_height: Option<f64>` (from `<sheetFormatPr defaultRowHeight>`), validates it, and calls `Sheet::set_row_heights` once.
- Rejects a batch larger than `MAX_ROW_HEIGHT_RANGES` before sorting (`Error::TooManyRowHeightRanges`), and rejects overlapping ranges after sorting by `min` (`Error::InvalidRowHeightRange`) — the same fail-closed policy as `resolve/column_width.rs`.
- **Explicitly out of scope**: building/compressing `RowHeightRange` from `<row r="N" ht="..">` attributes in the first place (`parse/worksheet.rs`'s `push_row_height` — the decisive difference from `resolve/column_width.rs`, see below), the binary-search lookup itself (`Sheet::row_height`, see [model/sheet.en.md](../model/sheet.en.md)).

## Key types / functions

```rust
use crate::error::Error;
use crate::model::{RowHeightRange, Sheet};

pub(crate) const MAX_ROW_HEIGHT_RANGES: usize = 2_000;

pub(crate) fn resolve(
    sheet: &mut Sheet,
    mut ranges: Vec<RowHeightRange>,
    default_row_height: Option<f64>,
) -> Result<(), Error> {
    if ranges.len() > MAX_ROW_HEIGHT_RANGES {
        return Err(Error::TooManyRowHeightRanges {
            count: ranges.len(),
            limit: MAX_ROW_HEIGHT_RANGES,
        });
    }

    ranges.sort_by_key(|r| r.min);
    for pair in ranges.windows(2) {
        let (prev, next) = (&pair[0], &pair[1]);
        if prev.max >= next.min {
            return Err(Error::InvalidRowHeightRange {
                min: next.min,
                max: next.max,
                reason: "overlaps another row height range".to_string(),
            });
        }
    }

    sheet.set_row_heights(ranges, default_row_height);
    Ok(())
}
```

Unlike `resolve::column_width::resolve`, there is no per-range `min > max` check. `parse/worksheet.rs`'s `push_row_height` always starts a range as `RowHeightRange { min: row, max: row, .. }` (min == max) and only ever extends `max` monotonically — a single range can never end up with `min > max` by construction, unlike column width, where a file can write an arbitrary `<col min="10" max="5">` directly.

## Relationship to `resolve/column_width.rs`

Both validate a batch of ranges Phase 3 collected, then register it in one call, and both get away with checking only adjacent pairs after sorting for O(R log R) overlap detection (a 1D interval problem, so `resolve/merge.rs`'s O(N²) rectangle check isn't needed — see [column_width.en.md](column_width.en.md) for why).

The decisive difference is **which layer owns the compression**:

- **Column width**: `<col min=".." max="..">` is itself a range in the OOXML schema, so `parse/worksheet.rs` just repackages what the file already wrote as `ColWidthRange` — no compression happens on this library's side at all (the file hands it over pre-compressed).
- **Row height**: `<row r="N" ht="..">` is always one element per row — there's no concept of a row range in the schema at all. A real fixture (an `.xlsx` skills-matrix template) had 1,000 individual `<row ht="..">` elements, which compressed down to just 32 ranges (31.2x) — meaning **without compression, row height would produce an entry count nowhere close to column width's**. Doing that compression on the `resolve/row_height.rs` side (receiving raw `(row, height_pt)` pairs and compressing them there) would let an intermediate buffer grow proportionally to the row count (measured: ~15.6 MB for 1 million rows). So the design instead compresses inline, right where `parse/worksheet.rs` streams `<sheetData>` row by row (`push_row_height`: extends the last range if the current row immediately follows it at the same height, otherwise starts a new one) — by the time `resolve::row_height::resolve` sees it, the list is already compressed, and this module stays focused on the same validate-and-register job column width has (measured: the same 1-million-row, single-height case costs only tens of bytes under streaming compression).

This design choice (streaming compression over buffer-then-compress) was settled through sister project exceldiff's [Issue #51](https://github.com/MinamiyamaKotaro/exceldiff/issues/51) PoC verification, including real heap-allocation measurement, and ported over to this crate as-is, the same way `model/sheet.rs` and `parse/worksheet.rs` were.

## Dependencies

- Depends on: [`model/sheet.rs`](../model/sheet.en.md) (`Sheet::set_row_heights`, `RowHeightRange`), [`error.rs`](../error.en.md)
- Depended on by: [`resolve/mod.rs`](mod.en.md) (called from `resolve_sheet`, after `column_width::resolve` and before `merge::resolve`), [`parse/worksheet.rs`](../parse/worksheet.en.md) (the side that supplies the already-compressed `row_height_ranges`/`default_row_height` as part of `WorksheetParseOutput`)

## Error handling policy

- A batch larger than `MAX_ROW_HEIGHT_RANGES`, or two overlapping ranges, are rejected as `Error::TooManyRowHeightRanges` / `Error::InvalidRowHeightRange { min, max, reason }` respectively. Overlap shouldn't normally occur (`push_row_height` builds the list in a single forward pass, so it's already sorted and non-overlapping) — this check exists defensively, for a file whose `<row>` elements aren't in ascending `r` order (malformed or adversarial input).
- Never panics.
- Registers nothing on validation failure (fail closed) — same policy as `resolve/column_width.rs`.

## Test plan

- Non-overlapping ranges register correctly regardless of input order (sorted internally).
- Overlapping ranges (including an exact duplicate) are rejected as `Error::InvalidRowHeightRange`.
- Adjacent, non-overlapping ranges are accepted.
- Exactly `MAX_ROW_HEIGHT_RANGES` ranges are accepted; one more is rejected as `Error::TooManyRowHeightRanges`.
- An empty range list still registers `default_row_height` correctly; with neither a covering range nor a default, `None` is returned.
- On the `parse/worksheet.rs` side (the compression itself): consecutive same-height rows compress into one range; a gap in row numbers (a row with no explicit `<row ht>` in between) starts a new range even at the same height; a row with no `ht` attribute contributes nothing.

## Open questions

1. ~~**Exposing this through JSON output (`json.rs`)**~~ **Resolved**: `row_height_ranges()`/`default_row_height()` are exposed symmetrically with column width's `col_width_ranges()`/`default_col_width()`, and `json.rs` serializes them as sheet-level `rows`/`defaultRowHeight` fields (never duplicated per cell).
2. **Handling of `customHeight`**: `<row ht="..">` is read regardless of whether `customHeight="1"` is set — a `ht` without it is Excel's own auto-calculated estimate rather than something the user explicitly chose, but either way it's what Excel actually renders, and this library aims to reproduce the visible result rather than which of the two the file's author picked.
