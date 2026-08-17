# xlsxparser

A lightweight, high-performance `.xlsx` (OOXML) parser library written in Rust.

## Motivation

`xlsxparser` aims to be a fast, low-memory alternative to existing Rust
spreadsheet crates (e.g. `umya-spreadsheet`), purpose-built for the kind of
`.xlsx` files common in Japanese business systems: sheets with an extreme
number of rows/columns ("方眼紙Excel") and heavy use of merged cells. The
goal is to parse and analyze such files without loading a full in-memory
grid, and to expose the result as JSON that's easy to consume from a
frontend or another system.

## Status

Early stage — the requirement spec has been drafted and implementation has
not started yet. See [docs/requirement/requirements.md](docs/requirement/requirements.md)
(Japanese) for the full architecture and functional requirements, summarized
below.

## Planned architecture

A one-way processing pipeline in five phases:

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
   consumption.

Core requirements driving the design:

- **Sparse storage** — cells are kept in a coordinate-keyed map, never a
  dense 2D array, so sparse "grid-paper" sheets stay cheap to hold in memory.
- **Merge-cell transparency** — any coordinate inside a merged range
  resolves (via an internal alias) to the same value and merge metadata as
  the range's anchor cell.

## OOXML parts covered

- `[Content_Types].xml`
- `xl/workbook.xml`
- `xl/sharedStrings.xml` (including `xml:space="preserve"` handling)
- `xl/styles.xml`
- `xl/worksheets/sheetX.xml` (`<sheetData>`, `<mergeCells>`)

## License

TBD.
