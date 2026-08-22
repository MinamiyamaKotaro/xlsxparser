# `src/` Architecture Design

*[日本語](architecture.md)*

A document summarizing the `src/` directory layout and the responsibilities of each module, finalized through the discussion in Issue [#1](https://github.com/MinamiyamaKotaro/xlsxparser/issues/1). It maps to the 5-phase pipeline defined in the requirements specification ([requirements.md](../requirement/requirements.md)).

## Design Principles

1. **Separation of responsibility by phase**: The one-directional pipeline — rels resolution → sanitization → streaming parse → analysis/deferred resolution → JSON generation — assigns each phase to a corresponding module on a one-to-one basis.
2. **Separation of I/O and domain logic**: The I/O layer (ZIP extraction, XML parsing — `container/` `parse/`) is separated from the domain logic (shared string resolution, merged cell resolution, style application — `resolve/`). `resolve/` has no dependency on I/O or XML structure at all, and operates purely on in-memory data structures such as `model::Sheet`, ensuring testability.
3. **Centralized orchestration**: `container` and `parse` actually go back and forth tightly, in the form of "get a byte stream from ZIP → parse it → fetch the next ZIP entry based on the result." This call ordering and the lifecycle management of resources (such as `ZipContainer`) is centralized in `pipeline.rs`, so that other modules don't need to know about each other directly.
4. **Naming convention**: `package` is avoided because it is easily confused with the Cargo package; `container`, which reflects the nature of OPC (Open Packaging Conventions), is used instead.

## Directory Layout

```text
src/
  lib.rs                  # Public API entry point (e.g. parse_workbook(path) -> Result<Workbook>)
  error.rs                # Common error definitions for the whole library
  pipeline.rs              # Orchestrator for phases 1-5 as a whole (I/O and lifecycle management)

  container/               # I/O & security guard
    mod.rs                # ZIP extraction entry point, safe file retrieval
    sanitize.rs           # Phase 2: Zip Bomb / Zip Slip detection logic

  parse/                    # XML parsing only (aggregates quick-xml-dependent code)
    mod.rs                # Common XML parser helpers, Phase 2: XXE disabling config (Reader initialization)
    relationships.rs      # Phase 1: _rels parsing (parsing data used to build the routing map)
    workbook.rs           # Parsing of workbook.xml
    shared_strings.rs     # Parsing of sharedStrings.xml (extracting structured SST data)
    styles.rs             # Parsing of styles.xml (fonts/fills/borders/numFmts/cellXfs)
    theme.rs              # Parsing of theme{N}.xml's <clrScheme> (Issue #76; read only when a style references a theme color)
    worksheet.rs          # Phase 3: SAX-style stream parse of sheetX.xml (per-row disposal completes here); also collects <mergeCells>/<cols>/<hyperlinks> (Issue #95) for Phase 4
    drawing.rs             # Phase 3.5: parses drawingN.xml's image anchors (Issue #65)

  model/                    # Pure domain model (no dependency on XML parsing or resolution logic)
    mod.rs
    cell.rs               # CellValue, Cell, CellRef (A1 notation <-> coordinates)
    sheet.rs              # Sparse matrix Sheet (BTreeMap<CellRef, Cell>), merged regions, column widths, images, hyperlinks
    workbook.rs           # Resolved Workbook model
    style.rs              # ResolvedStyle / StyleSheet / StyleId / Borders (shared vocabulary between parse/styles.rs and resolve/style.rs; added per PR #8 review feedback)
    color.rs              # Rgb / ThemePalette — the resolved-color types Issue #76's resolve_color returns

  resolve/                  # Phase 4: analysis and deferred resolution (I/O-independent, operates only on model::Sheet)
    mod.rs                # Entry point for Phase 4 resolution processing
    shared_strings.rs     # Index resolution of shared strings (SST)
    merge.rs              # Deferred resolution of merged cells / alias reference mapping
    style.rs              # Applying cell styles
    column_width.rs       # Deferred resolution of <cols> column-width ranges (Issue #39)
    hyperlink.rs          # Overlap validation + sweep-line resolution of <hyperlink> ranges (Issue #95)
    color.rs              # On-demand ColorRef -> Rgb resolution (Issue #76; not part of the per-cell pipeline — a display-oriented API called only when a caller needs it)

  json.rs                   # Phase 5: JSON serialization including row_span/col_span
```

## Module Responsibility Details

### `lib.rs`

The crate root. Defines the public API entry points (`parse_workbook(path) -> Result<Workbook>`, etc.) and declares `container/` `parse/` `resolve/` `pipeline.rs` `json.rs` as private `mod`s, hiding them as crate-internal implementation, while re-exporting outward only a subset of `model/`'s types plus `error::Error`.

- Detailed design: [lib.md](lib.en.md)

### `error.rs`

Defines the single `Error` enum shared across the entire crate. It is the most foundational leaf module, depending on no other module in the crate (including `model/`), and is depended on by nearly every module: `container/`, `parse/`, `model/`, `resolve/`, `pipeline.rs`, `lib.rs`.

- Detailed design: [error.md](error.en.md)

### `pipeline.rs`

Owns the `ZipContainer` and controls the execution order of each phase (borrowing a stream from `container`, passing it to `parse`, resolving the result in `resolve`, and serializing it in `json.rs`) and the timing of resource disposal.

- Disposes of `_rels` temporary buffers immediately after Phase 1 completes (once the routing map has been built).
- Disposes of `SharedStringTable` and `StyleSheet` once Phase 4 completes (once shared string and style resolution are done).
  Note: This disposal is only valid if `model::Cell` holds resolved actual data directly (the `String` or `ResolvedStyle` value itself, or an owned reference such as `Arc`) rather than an index. If the cell side holds only an index/reference, the lifetime of `SharedStringTable`/`StyleSheet` must be kept alive until Phase 5 (JSON generation) completes.
- Per-row XML node disposal (Phase 3) is an internal implementation detail of `parse/worksheet.rs`, and `pipeline.rs` does not control it. `pipeline.rs` is only responsible for file/data-structure-level disposal.

- Detailed design (per-module design docs in progress under Issue [#3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)): [pipeline.md](pipeline.en.md)

### `container/`

The entry point for ZIP (OPC) extraction. Responsible for detecting and blocking Zip Bomb / Zip Slip. Does not interpret (parse) the contents of the XML.

- Detailed design (per-module design docs in progress under Issue [#3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)): [mod.md](container/mod.en.md) / [sanitize.md](container/sanitize.en.md)

### `parse/`

The layer that aggregates dependencies on XML parsing libraries such as `quick-xml`. It only repackages XML elements into pure structs and holds no business logic (merged cell resolution, shared string resolution, etc.). Since disabling external entity expansion during XML parsing (XXE countermeasure) is a `Reader` initialization setting in quick-xml, it is the responsibility of this layer (`parse/mod.rs`), which aggregates the quick-xml dependency.

- Since each parser (`workbook.rs` / `worksheet.rs`, etc.) initializing its own `Reader` individually risks missing the configuration, `parse/mod.rs` provides a dedicated factory function for secure `Reader` creation (e.g. `create_secure_reader`) to enforce XXE countermeasures uniformly. Every module under `parse/` obtains its `Reader` only through this factory.

`parse/worksheet.rs` streams out row/cell data and `<mergeCells>`/`<cols>`/`<hyperlinks>` information sequentially. `parse/theme.rs` is read on a "pay for what you use" basis — only when `parse/styles.rs` actually resolved a style referencing a theme color (Issue #76).

- Detailed design (per-module design docs in progress under Issue [#3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)): [mod.md](parse/mod.en.md) / [relationships.md](parse/relationships.en.md) / [workbook.md](parse/workbook.en.md) / [shared_strings.md](parse/shared_strings.en.md) / [styles.md](parse/styles.en.md) / [theme.md](parse/theme.en.md) / [worksheet.md](parse/worksheet.en.md) / [drawing.md](parse/drawing.en.md)

### `model/`

Defines pure Rust data structures such as `Cell` / `Sheet` / `Workbook`. Has no dependency on XML parsing or resolution logic. Optimizes memory usage via a sparse matrix (`BTreeMap<(row, col), Cell>`). See [model/sheet.en.md](model/sheet.en.md) for why `BTreeMap` (changed from `HashMap`).

This sparse-matrix choice is what [README.md's Benchmarks section](../../README.md#benchmarks) measures directly against a dense-`Vec`-backed reader (`calamine`) on `tests/fixtures/complex/extreme_sparse.xlsx` — two cells at Excel's actual opposite corners, so a bounding-box-sized allocation is 17.18 billion elements. `exceldiff` costs exactly 2 map entries and finishes in single-digit milliseconds; the dense-array reader gets killed by the OS after climbing to multiple GB of resident memory. The README's resource-over-time plot (`docs/benchmarks/extreme_sparse_memory.svg`) makes this concrete rather than theoretical.

- Detailed design (per-module design docs in progress under Issue [#3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)): [mod.md](model/mod.en.md) / [cell.md](model/cell.en.md) / [sheet.md](model/sheet.en.md) / [workbook.md](model/workbook.en.md) / [style.md](model/style.en.md) / [color.md](model/color.en.md)

### `resolve/`

Responsible for Phase 4's analysis and deferred resolution. Since it has no dependency on I/O or XML structure, it can be unit tested using only in-memory data such as `model::Sheet`.

- `shared_strings.rs`: resolves `t="s"` indices to the actual strings in the `SharedStringTable`.
- `merge.rs`: after the stream completes, matches the merged-range list from `<mergeCells>` against the collected cell data, mapping alias references from virtual cell coordinates to their origin cell.
- `style.rs`: applies resolved formatting information from `styles.xml` to cells.
- `column_width.rs`: validates `<cols>` column-width ranges (start/end ordering, overlap, a count cap — Issue #39) and registers them onto `Sheet`.
- `hyperlink.rs`: validates `<hyperlink>` ranges the same way (`resolve::merge`'s overlap-validation shape) and resolves each already-populated covered cell to its hyperlink in a single sweep-line pass (Issue #95) — never a per-cell scan, the same complexity concern `Sheet::finalize_merges` addresses for merges.
- `color.rs`: not part of the per-cell pipeline above — `resolve_color` is a pure, on-demand function (`ColorRef` → `Rgb`) a caller invokes only when it actually needs a displayed color (Issue #76).

- Detailed design (per-module design docs in progress under Issue [#3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)): [mod.md](resolve/mod.en.md) / [shared_strings.md](resolve/shared_strings.en.md) / [merge.md](resolve/merge.en.md) / [style.md](resolve/style.en.md) / [column_width.md](resolve/column_width.en.md) / [hyperlink.md](resolve/hyperlink.en.md) / [color.md](resolve/color.en.md)

### `json.rs`

Serializes the fully analyzed and resolved data model into JSON, including attributes such as `row_span` / `col_span` needed for frontend rendering.

- Detailed design (per-module design docs in progress under Issue [#3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)): [json.md](json.en.md)

## Discussion History

For details on the validation of this layout and its incremental revisions, see the comment history of Issue [#1](https://github.com/MinamiyamaKotaro/xlsxparser/issues/1). The main points of discussion were:

- Renaming `package/` to `container/` (avoiding a naming collision with the Cargo package)
- Consolidating XML parsing code into `parse/` (hiding the underlying tech stack, improving testability)
- Clarifying where shared string resolution lives (`resolve/shared_strings.rs`)
- Introducing an orchestration layer (`pipeline.rs`) for the back-and-forth calls between `container` and `parse`
- Separating the granularity of per-row disposal (an internal detail of the `parse` layer) from per-file disposal (controlled by `pipeline.rs`)
- Moving the XXE disabling configuration from `container/sanitize.rs` to `parse/mod.rs` (Zip Bomb/Zip Slip is a threat at the ZIP layer while XXE countermeasures are an XML parser configuration concern — these are separate layers, and the latter should be aligned with the design principle of "aggregating the quick-xml dependency into `parse/`" (Design Principle 2))
