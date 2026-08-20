# xlsxparser

[![Rust CI](https://github.com/MinamiyamaKotaro/xlsxparser/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/MinamiyamaKotaro/xlsxparser/actions/workflows/rust-ci.yml)
[![Docs](https://github.com/MinamiyamaKotaro/xlsxparser/actions/workflows/docs.yml/badge.svg)](https://github.com/MinamiyamaKotaro/xlsxparser/actions/workflows/docs.yml)
[![xlsxparser on crates.io](https://img.shields.io/crates/v/xlsxparser.svg)](https://crates.io/crates/xlsxparser)
[![codecov](https://codecov.io/gh/MinamiyamaKotaro/xlsxparser/branch/master/graph/badge.svg)](https://codecov.io/gh/MinamiyamaKotaro/xlsxparser)
[![License](https://img.shields.io/github/license/MinamiyamaKotaro/xlsxparser)](LICENSE)

A lightweight, high-performance `.xlsx` (OOXML) parser library written in Rust.

## Motivation

`xlsxparser` aims to be a fast, low-memory `.xlsx` parser, purpose-built for
the kind of files common in Japanese business systems: sheets with an
extreme number of rows/columns ("方眼紙Excel") and heavy use of merged
cells. The
goal is to parse and analyze such files without loading a full in-memory
grid, and to expose the result as JSON that's easy to consume from a
frontend or another system.

## Status

Core implementation complete — every module in the planned architecture
below is implemented and tested against the design in `docs/design/`. The
public API (`parse_workbook`, `parse_workbook_reader`, `to_json_string`,
`to_json_writer`) is wired up in `src/lib.rs`.

```rust
let workbook = xlsxparser::parse_workbook("book.xlsx")?;
let json = xlsxparser::to_json_string(&workbook)?;
```

- [docs/requirement/requirements.en.md](docs/requirement/requirements.en.md) —
  the functional requirements and the 5-phase pipeline summarized below
  (also available in [Japanese](docs/requirement/requirements.md)).
- [docs/design/architecture.en.md](docs/design/architecture.en.md) — the
  overall `src/` directory layout, module responsibilities, and design
  principles (also available in [Japanese](docs/design/architecture.md)).
  It links out to a per-module design doc for every file, covering
  responsibility/scope, key types and function signatures, dependencies,
  error handling policy, testing strategy, and open questions — each doc
  written in both Japanese and English (`*.md` / `*.en.md`). Where
  implementation diverged from a design doc's draft (an external API
  detail settled differently than planned, a bug found while writing
  tests, etc.), the doc was updated in place to record what changed and why.

## Input / Output

**Input**: a `.xlsx` file, via one of two entry points —

- `parse_workbook(path)` — the common case, reads from a filesystem path.
- `parse_workbook_reader(reader)` — from anything `Read + Seek` (an
  in-memory buffer, a fully-read HTTP response body, ...), for callers that
  don't go through the filesystem.

Both return `Result<Workbook, Error>`, a fully resolved in-memory
representation of every sheet (visible, hidden, and veryHidden alike). Each
has a `_with_limits` variant taking an explicit `SizeLimits` to override the
default Zip Bomb caps (512 MiB per ZIP entry, 2 GiB cumulative).

**Output**: `to_json_string(&workbook)` / `to_json_writer(&workbook,
writer)` serialize the resolved `Workbook` into JSON shaped like this
(real output, from `tests/fixtures/complex/houganshi_merged.xlsx` — a
sheet with a single merged region, `A1:C3`, holding one text cell):

```json
{
  "sheets": [
    {
      "name": "Sheet1",
      "visibility": "visible",
      "maxRow": 3,
      "maxCol": 3,
      "defaultColumnWidth": null,
      "columns": [],
      "cells": [
        {
          "row": 1,
          "col": 1,
          "value": { "type": "text", "value": "houganshi" },
          "rowSpan": 3,
          "colSpan": 3
        }
      ]
    }
  ]
}
```

- `visibility` is `"visible"`, `"hidden"`, or `"veryHidden"` (from `<sheet
  state="...">`).
- `maxRow`/`maxCol` are the sheet's bounding box (the highest populated or
  merged coordinate) — not the OOXML `<dimension>` value, which isn't read
  at all.
- `columns` is the sheet's `<cols>` ranges (`{"min", "max", "width"}`,
  1-based and inclusive), each entry covering every column in that range —
  not one `columnWidth` value duplicated onto every cell, since that would
  multiply output size for no benefit (see
  [Sparse merged-cell arrangements](#sparse-merged-cell-arrangements)
  below for the same principle applied to merged cells). `defaultColumnWidth`
  is `<sheetFormatPr defaultColWidth="..">`'s value, or `null` if the
  workbook doesn't set one; a column not covered by any `columns` entry
  falls back to it. Neither fixture used in these two examples declares
  `<cols>`, so both show the empty/absent case.
- `cells` only contains populated coordinates: a blank cell in between is
  simply absent, never emitted as a `null`/`"empty"` entry (see
  [Motivation](#motivation)). Cells are ordered row-major, then
  column-major (matching reading order), regardless of the order they
  appear in the source XML — the sheet is backed by a `BTreeMap`, keyed
  on `(row, col)`.
- Each cell's `value` is tagged by `type`:
  `"number"` | `"text"` | `"boolean"` | `"error"` | `"dateTime"` |
  `"empty"` (a cell with formatting only, or a value JSON can't
  represent — `NaN`/`±Infinity`).
  `"dateTime"` serializes as ISO 8601 with no timezone designator or
  fractional seconds (e.g. `"2023-06-15T00:00:00"`; a date-only cell gets a
  midnight time component, since Excel itself doesn't distinguish
  date-only from date+time as a type).
- `rowSpan`/`colSpan` are present (and `> 1`) only on a merged region's
  anchor cell; every other coordinate inside the region resolves to that
  same anchor and is not emitted as a separate JSON cell.
- `style` is present only when the cell carries a resolved style at all
  (omitted entirely otherwise, not emitted as `"style": {}`):
  - `font`: `{"sizePt": 11.0, "bold": false}`.
  - `wrapText`: boolean.
  - `alignment`: the horizontal alignment as a string — `"general"` |
    `"left"` | `"center"` | `"right"` | `"fill"` | `"justify"` |
    `"centerContinuous"` | `"distributed"`. Always present (unlike
    `numberFormat` below, `"general"` is itself a meaningful value, not
    "nothing to report").
  - `numberFormat`: the resolved format code as a string (e.g. `"0%"`,
    `"yyyy-mm-dd"`), covering both the built-in numFmtId table
    (ECMA-376 §18.8.30) and custom `<numFmt>` codes. Omitted when the
    format is `"General"` (no special formatting to report).
  - `fillFgColor`/`fillBgColor`: the cell's fill color, tagged by `type`
    exactly as `<fgColor>`/`<bgColor>` specify it — `{"type": "rgb",
    "value": "FFFF0000"}` | `{"type": "theme", "value": {"index": 4,
    "tint": -0.25}}` | `{"type": "indexed", "value": 64}`. Kept in this
    raw, unresolved form rather than converted to a final displayed RGB
    value: xlsxparser's output is for diffing, so knowing *that* a fill
    color changed doesn't require knowing what it actually renders as.
    Omitted when the fill has no foreground/background color at all.

A second real example — every `CellValue` variant in one row
(`tests/fixtures/normal/basic_types.xlsx`; cells re-ordered by column here
for readability, since actual order is unspecified):

```json
{
  "sheets": [
    {
      "name": "Sheet1",
      "visibility": "visible",
      "maxRow": 1,
      "maxCol": 7,
      "defaultColumnWidth": null,
      "columns": [],
      "cells": [
        { "row": 1, "col": 1, "value": { "type": "text", "value": "日本語Text" } },
        { "row": 1, "col": 2, "value": { "type": "number", "value": 42.0 } },
        { "row": 1, "col": 3, "value": { "type": "number", "value": 19.99 } },
        {
          "row": 1, "col": 4,
          "value": { "type": "dateTime", "value": "2023-06-15T00:00:00" },
          "style": {
            "font": { "sizePt": 11.0, "bold": false },
            "wrapText": false,
            "alignment": "general",
            "numberFormat": "yyyy-mm-dd"
          }
        },
        { "row": 1, "col": 5, "value": { "type": "boolean", "value": true } },
        { "row": 1, "col": 6, "value": { "type": "boolean", "value": false } },
        { "row": 1, "col": 7, "value": { "type": "error", "value": "#N/A" } }
      ]
    }
  ]
}
```

(Column 4 is a date cell — its `numberFormat` comes from the cell's
`<xf numFmtId="...">`, resolved against `xl/styles.xml`'s built-in/custom
`<numFmt>` table; `openpyxl`'s default date format is `"yyyy-mm-dd"`.)

A third real example — a sheet that does declare `<cols>`
(`tests/fixtures/normal.rs`'s `column_widths()`: `<col min="1" max="3"
width="12.5"/>`, `<col min="5" max="5" width="30"/>`, and
`<sheetFormatPr defaultColWidth="9.1"/>`):

```json
{
  "maxRow": 1,
  "maxCol": 5,
  "defaultColumnWidth": 9.1,
  "columns": [
    { "min": 1, "max": 3, "width": 12.5 },
    { "min": 5, "max": 5, "width": 30.0 }
  ],
  "cells": [
    { "row": 1, "col": 1, "value": { "type": "number", "value": 1.0 } },
    { "row": 1, "col": 5, "value": { "type": "number", "value": 2.0 } }
  ]
}
```

Column 4 falls in the gap between the two `columns` ranges, so a cell there
(none exist in this example) would fall back to `defaultColumnWidth`
(9.1) rather than either range's `width`.

## Architecture

1. **Relationship resolution** — parse `_rels` parts to build a routing map
   from sheet `r:id` to worksheet file path, then discard the intermediate
   data immediately.
2. **Sanitization** — guard against zip bombs, zip-slip path traversal, and
   XXE before any untrusted content is parsed.
3. **Streaming parse** — a SAX-style reader processes `<sheetData>` one
   `<row>` at a time, without holding the sheet's full XML DOM in memory.
4. **Resolution** — shared strings (`t="s"`) and cell styles are resolved
   against the SST/stylesheet, and `<mergeCells>` ranges are resolved
   against the collected cells after the stream pass completes.
5. **JSON output** — the resolved data model is serialized to structured
   JSON (including `row_span`/`col_span` for merged cells) for downstream
   consumption, as a separate step from the primary `Workbook`-returning API.

Core requirements driving the design:

- **Sparse storage** — cells are kept in a coordinate-keyed map, never a
  dense 2D array, so sparse "grid-paper" sheets stay cheap to hold in memory.
- **Merge-cell transparency** — any coordinate inside a merged range
  resolves (via an O(1) bounding-box pre-check plus a geometric containment
  scan over the sheet's merged regions) to the same value and merge
  metadata as the range's anchor cell.
- **I/O and domain logic stay separated** — XML/ZIP handling (`container/`,
  `parse/`) never mixes with the resolution logic (`resolve/`), which
  operates purely on in-memory data and needs no I/O to unit test.

The module layout (see
[docs/design/architecture.en.md](docs/design/architecture.en.md) for the
full breakdown of each file's responsibility):

```text
src/
  lib.rs        # public API entry point (parse_workbook, parse_workbook_reader, to_json_string, ...)
  error.rs      # crate-wide error type
  pipeline.rs   # orchestrates the 5-phase pipeline and resource lifetimes

  container/    # ZIP (OPC) extraction, zip-bomb/zip-slip guarding
  parse/        # XML parsing (quick-xml usage is confined here), XXE mitigation
  model/        # pure data structures (Workbook, Sheet, Cell, CellValue, ...)
  resolve/      # shared-string/style/merge-cell resolution, I/O-independent

  json.rs       # serializes a resolved Workbook to JSON
```

## OOXML parts covered

- `xl/_rels/workbook.xml.rels`
- `xl/workbook.xml` (including `<workbookPr date1904="...">`, needed to
  resolve a date/time cell's serial value under the 1900 vs. 1904 date
  system)
- `xl/sharedStrings.xml` (rich-text run concatenation, `xml:space="preserve"`
  handling, CDATA runs, and the `_x000D_` escape Excel uses for a literal CR)
- `xl/styles.xml` (font size/bold, horizontal alignment, wrap text,
  number format — both the built-in numFmtId table (ECMA-376 §18.8.30) and
  custom `<numFmt>` codes — and fill color, kept in its raw `rgb`/
  `theme`+`tint`/`indexed` form rather than resolved to a final RGB value)
- `xl/worksheets/sheetX.xml` (`<sheetData>`, `<mergeCells>`)

`[Content_Types].xml` is not read; fixed paths such as `xl/workbook.xml`
are accessed directly instead of being resolved through its Content-Type
declarations (see
[pipeline.en.md Open Question 3](docs/design/pipeline.en.md) for the
rationale and the strict-OPC-conformance tradeoff this makes).

## Benchmarks

The benchmarking was done using [`hyperfine`](https://github.com/sharkdp/hyperfine)
with `--warmup 3` on an `Apple M2 Pro` running `macOS 26.6.1`, comparing
`xlsxparser` (via `parse_workbook`) against
[`calamine`](https://github.com/tafia/calamine) `0.26.1` (via
`worksheet_range`) — a widely-used pure-Rust `.xlsx` reader — both built in
release mode, on `tests/fixtures/complex/extreme_sparse.xlsx`: a real,
openpyxl-authored file where only two cells are populated, `A1` and
`XFD1048576` (Excel's actual maximum: row 1,048,576, column 16,384) — the
sparse "grid-paper Excel" shape this library is purpose-built for (see
[Motivation](#motivation)).

```bash
xlsxparser
  Time (mean ± σ):       3.0 ms ±   1.0 ms    [User: 1.3 ms, System: 1.1 ms]
  Range (min … max):     2.1 ms …  18.3 ms    410 runs
```

`calamine` isn't shown as a completed hyperfine run because it never
completed one: across repeated runs it was killed by the OS for excessive
memory use after roughly 23-24 seconds, having grown to multiple GB of
resident memory. The cause is structural, not a fluke: `calamine`'s
`Range<T>` (the type `worksheet_range` returns) always backs onto a single
dense `Vec<T>` sized to the *bounding box* of the populated cells —
`Range::from_sparse` (`calamine` `0.26.1`, `src/lib.rs`) computes
`cols * rows` from that bounding box and allocates `vec![T::default();
cols * rows]` regardless of how few cells are actually non-empty. Here the
two populated corners span the full sheet, so that bounding box *is*
1,048,576 x 16,384 = 17,179,869,184 elements, and the allocation attempt
is what gets the process killed.

`xlsxparser` doesn't hit this because cells are kept in a coordinate-keyed
`BTreeMap<CellRef, Cell>` (see [Architecture](#architecture) above) sized to
the number of populated cells, never to the sheet's addressable bounding
box — so `extreme_sparse.xlsx` costs `xlsxparser` exactly 2 map entries.

### Sparse merged-cell arrangements

A merge-heavy file could hit an unrelated cost even while respecting every
existing limit ([Issue #43](https://github.com/MinamiyamaKotaro/xlsxparser/issues/43)):
two 1x1 merges placed at opposite corners of a sheet stretch the merged-cell
bounding box to cover virtually the whole sheet, so every other cell fell
back to a linear scan over every merged region when resolving its origin —
turning a legitimate file into an O(cells × merged regions) cost during JSON
generation. `Sheet::finalize_merges` closes this with a single sweep-line
pass, independent of how the merges are arranged in space (see
[docs/design/model/sheet.md](docs/design/model/sheet.md)'s "修正:
`finalize_merges`" section for the full story).

Measured the same way as above (`hyperfine`, `--warmup 1`, same machine), on
a generated 838 KB file with 300,000 distinct populated cells and 20,000
merges (`resolve::merge::MAX_MERGE_REGIONS`, the current cap) arranged to
maximize the bounding box (`tests/fixtures/security.rs`'s
`sparse_merge_bounding_box_amplification`):

```bash
before (pre-#43 fix, v0.10.0)
  Time (mean ± σ):     14.918 s ±  0.242 s    3 runs

after (this fix, v0.10.1)
  Time (mean ± σ):     600.6 ms ±   7.9 ms    4 runs
```

## Security notes

- **Zip Bomb / Zip Slip / XXE**: guarded against at parse time (see
  [Architecture](#architecture) above and
  [docs/security/design-review.md](docs/security/design-review.md) for the
  full analysis).
- **CSV / formula injection**: cell string values (including formula-computed
  result strings) pass through unchanged, with no escaping at any stage —
  this is safe as JSON output, but callers who re-export parsed values into
  CSV or another spreadsheet format are responsible for their own
  formula-injection mitigations (e.g. escaping a value that starts with `=`,
  `+`, `-`, or `@`), since a `.xlsx` input is untrusted and this library
  performs no rewriting of cell content.

## License

This project is licensed under the GNU Affero General Public License v3.0 (AGPL-3.0). See the [LICENSE](LICENSE) file for details.

### Commercial Licensing

If you wish to use this software in a proprietary system or without the copyleft obligations of the AGPL-3.0, commercial licenses are available.

Please contact the author via [GitHub Profile](https://github.com/MinamiyamaKotaro) or open an inquiry on [GitHub Discussions](https://github.com/MinamiyamaKotaro/xlsxparser/discussions).
