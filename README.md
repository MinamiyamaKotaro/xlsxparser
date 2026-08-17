# xlsxparser

[![Rust CI](https://github.com/MinamiyamaKotaro/xlsxparser/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/MinamiyamaKotaro/xlsxparser/actions/workflows/rust-ci.yml)
[![Docs](https://github.com/MinamiyamaKotaro/xlsxparser/actions/workflows/docs.yml/badge.svg)](https://github.com/MinamiyamaKotaro/xlsxparser/actions/workflows/docs.yml)

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

- [docs/requirement/requirements.md](docs/requirement/requirements.md)
  (Japanese) — the functional requirements and the 5-phase pipeline summarized below.
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
- `xl/workbook.xml`
- `xl/sharedStrings.xml` (including `xml:space="preserve"` handling)
- `xl/styles.xml`
- `xl/worksheets/sheetX.xml` (`<sheetData>`, `<mergeCells>`)

`[Content_Types].xml` is not read; fixed paths such as `xl/workbook.xml`
are accessed directly instead of being resolved through its Content-Type
declarations (see
[pipeline.en.md Open Question 3](docs/design/pipeline.en.md) for the
rationale and the strict-OPC-conformance tradeoff this makes).

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

MIT — see [LICENSE](LICENSE).
