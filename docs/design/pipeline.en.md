# `pipeline.rs` Design Doc

*[日本語](pipeline.md)*

Design doc for `src/pipeline.rs`. This is the orchestrator for the 5-phase pipeline [architecture.md](architecture.en.md) defines — it wires together [`container/`](container/mod.en.md), [`parse/`](parse/mod.en.md), [`resolve/`](resolve/mod.en.md), and [`model/`](model/mod.en.md) in call order and controls resource lifetimes. This is the central file implementing architecture.md design policy 3: "centralize the tight back-and-forth between `container` and `parse`, and resource lifecycle management, in `pipeline.rs`."

## Responsibility / Scope

- Owns a [`container::ZipContainer`](container/mod.en.md) and calls `get_entry` sequentially throughout Phases 1–4 (fully consuming one entry before fetching the next — following the sequential-access pattern [container/mod.md](container/mod.en.md) already enforces via `get_entry`'s type signature)
- Forwards the `SizeLimits` ([lib.md](lib.en.md)) received from the caller (`lib.rs`) into `with_max_entry_size` / `with_max_total_size` ([container/mod.md](container/mod.en.md)) right after `ZipContainer::open_reader`, so callers can override the Zip Bomb size caps (security review Finding 2, Issue [#14](https://github.com/MinamiyamaKotaro/xlsxparser/issues/14)). The same `SizeLimits`'s `max_cells_per_sheet` is forwarded as-is into each sheet's `parse::parse_worksheet` call (Phase 3) (Issue [#88](https://github.com/MinamiyamaKotaro/xlsxparser/issues/88); see [container/sanitize.md](container/sanitize.en.md))
- **Phase 1**: first fetches and parses the package root's own relationship part, `_rels/.rels` (the one path OPC itself fixes, unlike `xl/_rels/workbook.xml.rels`), and resolves the actual workbook part path from whatever the `officeDocument` relationship type targets (Issue [#55](https://github.com/MinamiyamaKotaro/xlsxparser/issues/55)). This used to read `xl/workbook.xml` as a hardcoded path; but that fixedness is not an OPC guarantee, and a real file was found (`tests/fixtures/other/minimal_package.xlsx`, from calamine's test corpus) whose workbook part is discoverable only via `_rels/.rels`, so resolution was changed to go through `_rels/.rels` → `officeDocument` relationship → workbook part, as OPC intends. The workbook part's own `_rels` path (the `xl/_rels/workbook.xml.rels`-equivalent) is likewise derived from this resolved workbook part path via `rels_path_for` (see "Key Types / Functions (draft)" below) — it no longer depends on the literal string `xl/workbook.xml`. It then fetches and parses that workbook part's `_rels` and its `workbook.xml` body, building a "routing plan" of sheet names, visibility, and backing file paths. It also identifies the relationships to `sharedStrings.xml` / `styles.xml` within the workbook part's `_rels` by relationship type (`Relationship.rel_type`) — implementing the division of labor [relationships.md Not Responsible For](parse/relationships.en.md) assigned to the caller: "which `r:id` corresponds to which part kind is the caller's job". A workbook may have neither relationship (`sharedStrings.xml` has always been treated as optional; `styles.xml` joined it as of Issue #54 — see Error Handling Policy). This phase also reads `ParsedWorkbookXml::date1904` (Issue #40) and holds it in a local variable — it never becomes a field on `Workbook`, following the same "phase-transient value" treatment `StyleSheet` gets (see below). It used to be passed only to Phase 4's `resolve::resolve_sheet`; with `t="d"` support (Issue #58 / PR #80 review point 2), the same value is now also passed into each sheet's `parse::parse_worksheet` call (Phase 3) — see [parse/worksheet.md](parse/worksheet.en.md)
- Once the routing plan is built, lets the reader used for the rels read and the [`parse::RelationshipMap`](parse/relationships.en.md) go out of scope and be dropped (implements architecture.md's "dispose of the `_rels` scratch buffer immediately once the routing map is built at the end of Phase 1")
- Once the routing plan is finalized, builds the [`SharedStringTable`](parse/shared_strings.en.md) and [`StyleSheet`](model/style.en.md) exactly once, before entering the per-sheet loop
- For each sheet, builds an empty sheet via [`model::Sheet::new`](model/sheet.en.md), passes the corresponding entry to [`parse::parse_worksheet`](parse/worksheet.en.md) to stream cells into it (Phase 3), then passes that output to [`resolve::resolve_sheet`](resolve/mod.en.md) to resolve it (Phase 4)
- **Phase 3.5** (Issue [#65](https://github.com/MinamiyamaKotaro/xlsxparser/issues/65)): if Phase 3 collected a `<drawing r:id="...">` for this sheet, resolves it end to end into `Vec<model::Image>` — reads the worksheet's own `_rels` to locate `drawingN.xml`, parses it via [`parse::parse_drawing`](parse/drawing.en.md), then reads `drawingN.xml`'s own `_rels` to resolve each `<xdr:pic>`'s `r:embed`/hyperlink `r:id` into a target path, verifying an `Internal` target's existence in the ZIP via [`ZipContainer::has_entry`](container/mod.en.md) without ever reading its bytes. `r:embed` failing to resolve is fail-fast (`Error::DanglingRelationship`); the image's own hyperlink failing to resolve degrades to `Image::hyperlink: None` instead (PR #66 review — see `resolve_sheet_images`'s doc comment for the asymmetry's rationale). Kept here rather than in `resolve/` since every step does ZIP I/O — see [drawing.md](parse/drawing.en.md) Dependencies. Runs between Phase 3 and Phase 4 but has no data dependency on either (`resolve_sheet` never touches `Sheet::images`), so the exact interleaving is not load-bearing
- **Phase 3.5, cell hyperlinks** (Issue #95): after `resolve::resolve_sheet` (Phase 4, which finalizes merges) runs, if Phase 3 collected any `pending_hyperlinks` for this sheet, resolves each `r_id` (if present) against the worksheet's own `_rels` into a raw Target string — loading `_rels` at all only when at least one pending entry actually carries an `r_id` (a `location`-only sheet never touches the ZIP for this step). Unlike Phase 3.5's image resolution, an `Internal` target's ZIP-entry existence is deliberately **not** verified, and an External URL is never touched beyond copying its string — Issue #95's explicit scope decision that hyperlinks are captured for diffing, never followed. An `r_id` absent from `_rels` (a malformed file) degrades to `target: None` rather than failing the sheet, the same tradeoff `Image::hyperlink` already makes. The resulting `Vec<model::HyperlinkRange>` is then handed to [`resolve::hyperlink::resolve`](resolve/hyperlink.en.md) (validation + `Sheet::finalize_hyperlinks`, both pure) in one call — unlike images, this step's own ZIP I/O is a self-contained helper (`resolve_sheet_hyperlinks`) that only *builds* the batch; registering it into `Sheet` is left to the (I/O-independent) `resolve::hyperlink::resolve` rather than done here directly, since that validation is real domain logic, not I/O plumbing
- Once every sheet has been processed, lets [`SharedStringTable`](parse/shared_strings.en.md) and [`StyleSheet`](model/style.en.md) go out of scope and be dropped, then builds the final model via [`model::Workbook::new`](model/workbook.en.md) and returns it
- **Not responsible for**: the logic of any individual phase (ZIP extraction/sanitization is `container/`, XML structure interpretation is `parse/`, semantic resolution is `resolve/`), row-level XML node disposal (an internal detail of Phase 3, owned by `parse/worksheet.rs` — per architecture.md, "`pipeline.rs` does not control this"), JSON generation itself ([`json.rs`](json.en.md) — see Open Question 1 for where calling it fits in the design)

## Key Types / Functions (draft)

```rust
use crate::container::sanitize::SizeLimits;
use crate::container::ZipContainer;
use crate::error::Error;
use crate::model::sheet::{Sheet, SheetVisibility};
use crate::model::style::StyleSheet;
use crate::model::workbook::Workbook;
use crate::parse::shared_strings::SharedStringTable;
use crate::{container, model, parse, resolve};
use std::io::{Read, Seek};

// The package root's own relationship part. Unlike xl/workbook.xml, this is
// the one path OPC itself fixes (Issue #55).
const PACKAGE_RELS_PATH: &str = "_rels/.rels";
const OFFICE_DOCUMENT_REL_TYPE_SUFFIX: &str = "/relationships/officeDocument";
const SHARED_STRINGS_REL_TYPE_SUFFIX: &str = "/relationships/sharedStrings";
const STYLES_REL_TYPE_SUFFIX: &str = "/relationships/styles";

/// One sheet's routing info, finalized once Phase 1 completes.
struct SheetRoute {
    name: String,
    visibility: SheetVisibility,
    /// A ZIP-entry-name-equivalent absolute path, ready to pass directly to
    /// `container::get_entry`.
    worksheet_path: String,
}

/// Runs Phases 1 through 5 end to end and returns the fully resolved
/// `Workbook`. `lib.rs`'s public API (`parse_workbook`, etc. — see Open
/// Question 2) calls this function. Generic over `Read + Seek` because it
/// simply carries forward [container/mod.md](container/mod.en.md)'s
/// `ZipContainer::open_reader` constraint (reading the ZIP central
/// directory requires a seekable input). `limits` is the Zip Bomb size cap
/// ([lib.md](lib.en.md)'s `SizeLimits`) — `lib.rs`'s `parse_workbook` /
/// `parse_workbook_reader` pass `SizeLimits::default()`, while
/// `parse_workbook_with_limits` / `parse_workbook_reader_with_limits` pass
/// the caller-supplied value straight through.
pub(crate) fn run<R: Read + Seek>(reader: R, limits: SizeLimits) -> Result<Workbook, Error> {
    let mut container = ZipContainer::open_reader(reader)?
        .with_max_entry_size(limits.max_entry_size)
        .with_max_total_size(limits.max_total_size);

    // --- Phase 1: relationship resolution and building the routing plan ---
    // First read the package root's own _rels/.rels, and resolve the
    // workbook part's actual path from its officeDocument relationship
    // (Issue #55 — see body for details).
    let package_rels_reader = container
        .get_entry(PACKAGE_RELS_PATH)?
        .ok_or_else(|| Error::MissingRelationshipPart(PACKAGE_RELS_PATH.to_string()))?;
    let package_relationships =
        parse::parse_relationships(package_rels_reader, "", PACKAGE_RELS_PATH)?;
    let workbook_path = package_relationships
        .values()
        .find(|r| r.rel_type.ends_with(OFFICE_DOCUMENT_REL_TYPE_SUFFIX))
        .map(|r| r.target.clone())
        .ok_or_else(|| Error::InvalidPackage(PACKAGE_RELS_PATH.to_string()))?;
    drop(package_relationships);

    let (workbook_rels_path, workbook_dir) = rels_path_for(&workbook_path);
    let rels_reader = container
        .get_entry(&workbook_rels_path)?
        .ok_or_else(|| Error::MissingRelationshipPart(workbook_rels_path.clone()))?;
    let relationships =
        parse::parse_relationships(rels_reader, workbook_dir, &workbook_rels_path)?;

    let workbook_reader = container
        .get_entry(&workbook_path)?
        .ok_or_else(|| Error::InvalidPackage(workbook_path.clone()))?;
    let parsed_workbook = parse::parse_workbook_xml(workbook_reader, &workbook_path)?;
    let date1904 = parsed_workbook.date1904;

    let mut routes = Vec::with_capacity(parsed_workbook.sheets.len());
    for entry in parsed_workbook.sheets {
        let rel = relationships
            .get(&entry.r_id)
            .ok_or_else(|| Error::DanglingRelationship { r_id: entry.r_id.clone() })?;
        routes.push(SheetRoute {
            name: entry.name,
            visibility: entry.visibility,
            worksheet_path: rel.target.clone(),
        });
    }
    let shared_strings_path = relationships
        .values()
        .find(|r| r.rel_type.ends_with(SHARED_STRINGS_REL_TYPE_SUFFIX))
        .map(|r| r.target.clone());
    // A workbook that applies no cell styling at all is not required by
    // OOXML to carry a styles.xml part (Issue #54) — real third-party
    // writers have been observed to omit it entirely, and both Excel and
    // other readers accept such files by falling back to no styling rather
    // than rejecting the package.
    let styles_path = relationships
        .values()
        .find(|r| r.rel_type.ends_with(STYLES_REL_TYPE_SUFFIX))
        .map(|r| r.target.clone());

    // The reader used for the rels read, and the RelationshipMap, go out of
    // scope and are dropped here (implements architecture.md's "dispose of
    // the _rels scratch buffer at the end of Phase 1").
    drop(relationships);

    // --- Shared tables, built exactly once between Phases 1-3 ---
    let shared_string_table = match shared_strings_path {
        Some(path) => {
            let reader = container
                .get_entry(&path)?
                .ok_or_else(|| Error::InvalidPackage(path.clone()))?;
            parse::parse_shared_strings(reader, &path)?
        }
        // sharedStrings.xml itself is an optional OOXML part (may be
        // omitted for a workbook with no string cells at all).
        None => SharedStringTable::default(),
    };
    let stylesheet = match styles_path {
        Some(path) => {
            let reader = container
                .get_entry(&path)?
                .ok_or_else(|| Error::InvalidPackage(path.clone()))?;
            parse::parse_styles(reader, &path)?
        }
        // The relationship itself is absent — genuinely no styles.xml part
        // (as opposed to a relationship pointing at a missing entity, which
        // stays Error::InvalidPackage above). No cell can reference a
        // StyleId that was never assigned, so an empty StyleSheet degrades
        // gracefully rather than erroring (Issue #54).
        None => StyleSheet::new(),
    };

    // --- Per sheet: Phase 3 (streaming parse) -> Phase 4 (resolution) ---
    let mut sheets = Vec::with_capacity(routes.len());
    for route in routes {
        let mut sheet = Sheet::new(route.name, route.visibility);
        let reader = container.get_entry(&route.worksheet_path)?.ok_or_else(|| {
            Error::DanglingRelationship { r_id: route.worksheet_path.clone() }
        })?;
        let output = parse::parse_worksheet(
            reader,
            &route.worksheet_path,
            &mut sheet,
            date1904,
            limits.max_cells_per_sheet,
        )?;
        resolve::resolve_sheet(
            &mut sheet,
            &output.pending_shared_strings,
            &shared_string_table,
            &output.pending_styles,
            &stylesheet,
            date1904,
            output.merge_regions,
        )?;
        sheets.push(sheet);
    }
    // shared_string_table / stylesheet go out of scope and are dropped here
    // (implements architecture.md's "dispose of SharedStringTable and
    // StyleSheet once Phase 4 completes").

    Ok(Workbook::new(sheets))
}
```

## Dependencies

- Depends on: [`container/mod.rs`](container/mod.en.md) (`ZipContainer`), [`parse/mod.rs`](parse/mod.en.md) (`parse_relationships`, `parse_workbook_xml`, `parse_shared_strings`, `parse_styles`, `parse_worksheet`, `SharedStringTable`), [`resolve/mod.rs`](resolve/mod.en.md) (`resolve_sheet`), [`model/sheet.rs`](model/sheet.en.md) (`Sheet::new`, `SheetVisibility`), [`model/workbook.rs`](model/workbook.en.md) (`Workbook::new`), [`error.rs`](error.en.md)
- Depended on by: `lib.rs` (called from the public API — see Open Question 2)

`run`'s implementation always fully finishes with one entry before calling `get_entry` for the next — this is not incidental. [container/mod.md](container/mod.en.md)'s `get_entry` requires `&mut self` and ties the returned value's lifetime to that borrow (`impl Read + '_`), so the Rust borrow checker statically forbids holding multiple entries open for processing at the same time. `pipeline.rs`'s sequential control flow follows naturally from that type constraint (it is exactly the access pattern — "read rels → read SST → read worksheets one after another" — that [container/mod.md Open Question 2's resolution](container/mod.en.md) already assumed).

## Error Handling Policy

- Each phase's failure short-circuits via `?`; no later phase runs (the same fail-closed principle as [resolve/mod.md](resolve/mod.en.md)'s `resolve_sheet`). If even one sheet fails to parse or resolve, `run` never returns a `Workbook`, even a partial one covering the sheets already processed successfully (never silently returns a partially broken book — see Open Question 4)
- Which `Error` variant to construct from `Ok(None)` (a missing entry) returned by `container::get_entry` is this file's responsibility, per [container/mod.md](container/mod.en.md)'s statement that "only the caller's context can tell":
  - `_rels/.rels` (the package root's own relationship part) absent → `Error::MissingRelationshipPart` (the first mandatory part Phase 1 reads. Issue #55)
  - `_rels/.rels` exists but carries no `officeDocument`-typed relationship → `Error::InvalidPackage` (the workbook part's path cannot be resolved at all. Issue #55)
  - the workbook part's `_rels` (path derived from the `officeDocument` relationship's target) absent → `Error::MissingRelationshipPart`
  - the workbook part itself (same derived path) absent, or the `styles.xml`/`sharedStrings.xml` part a *relationship* points to is absent → `Error::InvalidPackage` (the relationship promised a part that isn't there — a corrupt or truncated package)
  - the r:id a `workbook.xml` `<sheet r:id="...">` points to is not found in the `RelationshipMap`, or the worksheet part a relationship points to is absent → `Error::DanglingRelationship`
- If no relationship for `sharedStrings.xml` *or* `styles.xml` is found at all — as opposed to a relationship existing but its target entity being missing, the case above — this is not an error: `sharedStrings.xml` falls back to `SharedStringTable::default()` (an empty table), and `styles.xml` falls back to `StyleSheet::new()` (an empty table), since both are optional OOXML parts a workbook with no string cells / no cell styling respectively is not required to carry. `styles.xml` joined this graceful-degradation treatment as of Issue #54 — real third-party `.xlsx` writers have been observed to omit it entirely for unstyled workbooks, and rejecting such an otherwise-valid package was an unnecessarily strict reading of the spec (verified against calamine's own real-file test corpus, which includes exactly this case)

## Testing Strategy

- Verify that a minimal, valid `.xlsx`-shaped ZIP (one sheet, containing numbers, shared-string references, and a merged cell) passed to `run` returns `Ok` with the expected `Workbook` (an integration test)
- Verify that a ZIP with no `_rels/.rels` returns `Error::MissingRelationshipPart` (Issue #55)
- Verify that a ZIP whose `_rels/.rels` carries no `officeDocument`-typed relationship returns `Error::InvalidPackage` (Issue #55)
- Verify that `run` still resolves correctly when the workbook part lives at the package root (`workbook.xml`) rather than `xl/workbook.xml`, discoverable only via `_rels/.rels` (a real-world example of exactly this is `tests/fixtures/other/minimal_package.xlsx`, from calamine's test corpus — that directory is `.gitignore`d and not part of this repo, so the integration test itself reproduces the same shape via a hand-authored fixture through `tests/fixtures/builder.rs`. Issue #55)
- Verify that a ZIP with no `xl/_rels/workbook.xml.rels` returns `Error::MissingRelationshipPart`
- Verify that a ZIP with no `xl/workbook.xml` returns `Error::InvalidPackage`
- Verify that returns `Error::DanglingRelationship` when a `workbook.xml` `<sheet r:id="...">`'s r:id is not found in the rels
- Verify that returns `Error::InvalidPackage` when the entity file the styles relationship in rels points to is absent from the ZIP
- Verify that returns `Error::DanglingRelationship` when the entity file a worksheet relationship points to is absent from the ZIP
- **Verify that a ZIP with no relationship of type `.../relationships/styles` at all (as opposed to a relationship whose target entity is missing) still resolves successfully, falling back to an empty `StyleSheet`** (Issue #54 — distinct from the `Error::InvalidPackage` case immediately above)
- Verify that a book with no `sharedStrings.xml` part at all (no string cells whatsoever) still completes successfully with an empty `SharedStringTable`, rather than erroring
- Verify that for a book with multiple sheets, each ends up in `Workbook.sheets()` in the order `xl/workbook.xml`'s `<sheets>` defines them (wiring to [model/workbook.md](model/workbook.en.md)'s source-order policy)
- Verify that if a later sheet (e.g. the second) fails to parse, the whole call returns `Err` rather than a `Workbook`, even though the first sheet was processed successfully (a regression test for fail-closed behavior)
- Verify that a book containing `Hidden`/`VeryHidden` sheets still includes every sheet in `Workbook`, none excluded (wiring to [model/workbook.md Open Question 1](model/workbook.en.md))
- Verify that passing `run` a `SizeLimits` whose `max_entry_size` is smaller than `DEFAULT_MAX_UNCOMPRESSED_SIZE` turns an input that would otherwise succeed into `Error::ZipBombDetected` (a wiring test confirming `SizeLimits` actually reaches `ZipContainer`; `with_max_entry_size`/`with_max_total_size`'s own logic is verified under [container/mod.md](container/mod.en.md))
- Verify that passing `run` a `SizeLimits` with a small `max_cells_per_sheet` turns the parse into `Error::TooManyCells` (a wiring test confirming `SizeLimits.max_cells_per_sheet` actually reaches `parse::parse_worksheet`; the actual counting/cutoff logic itself is [parse/worksheet.md](parse/worksheet.en.md)'s responsibility. Issue #88)

## Open Questions

1. **Whether `json.rs` is called from within `run`**: [architecture.md](architecture.en.md)'s `pipeline.rs` section describes the full 5-phase flow as "serializing the result resolved by `resolve` via `json.rs`," while [model/workbook.md](model/workbook.en.md) already explicitly states that `Workbook` (structured data, not JSON) "is exactly what the public API in `lib.rs` (`parse_workbook(path) -> Result<Workbook>`) returns." This design resolves that tension by reading architecture.md's description as a conceptual summary of "the 5-phase capability the crate provides as a whole," rather than a literal claim that a single function chains all five phases. `run` itself owns Phases 1–4 (returning `Workbook`); Phase 5 (JSON conversion) is a separate function provided by [`json.rs`](json.en.md), explicitly invoked on a `Workbook` — a two-step design. How `lib.rs` (not yet designed) exposes this two-step flow (separate `parse_workbook` and `parse_workbook_json` functions, a `to_json` method on `Workbook`, etc.) is to be settled when `lib.rs` is designed.
2. **Wiring with `lib.rs`**: `run` is `pub(crate)`, with a thin wrapper in `lib.rs`'s public API assumed to open a `std::fs::File` from a path string and pass it in. How far `lib.rs`'s public API should accept arbitrary `Read + Seek` input (e.g. an in-memory buffer) beyond file paths is to be settled when `lib.rs` is designed.
3. ~~Reliance on hardcoded `xl/workbook.xml` / `xl/_rels/workbook.xml.rels` paths~~ → **Partially resolved** (Issue [#55](https://github.com/MinamiyamaKotaro/xlsxparser/issues/55)): the workbook part's path is now resolved from `_rels/.rels`'s `officeDocument` relationship (see body). That resolution, however, relies solely on `Relationship.rel_type` (a prefix match on the relationship-type string) and still never references `[Content_Types].xml`'s Content-Type declarations at all. Strict OPC conformance could argue for cross-checking via `[Content_Types].xml` too (confirming a part's declared Content-Type matches what's expected), but relationship-type-based resolution alone has proven sufficient in practice — it resolves every case observed so far, including `minimal_package.xlsx` — so this remains low priority.
4. **Resilience to an individual sheet's parse failure**: currently, if even one sheet errors, the whole `run` call returns `Err` (fail closed). The requirements have no "skip broken sheets and return the rest" requirement, so this design was adopted, but it would need revisiting if an error-tolerant mode (returning whatever could be read) is ever required.
5. **Concurrency**: currently processes sheets one at a time, sequentially (naturally matching `container::get_entry`'s sequential-access constraint, as discussed under Dependencies). The requirements have no parallelism requirement, but there may be room, as a performance optimization for large multi-sheet books, to read every sheet's bytes into memory up front and process them in parallel via a thread pool (which would trade off against the streaming policy) — to be reconsidered based on post-implementation profiling.
6. ~~`BufRead` requirement vs. `container::get_entry`'s return type~~ → **Resolved at implementation time**: every `parse::parse_*` function requires `impl BufRead` (quick-xml's `Reader::read_event_into` needs it), but `container::get_entry` returns `BoundedReader<'_, impl Read + '_>`, which only implements `Read`. `run` wraps every reader it gets from `get_entry` in `std::io::BufReader::new(..)` before passing it to a `parse::parse_*` call. The draft code block above predates this and omits the wrapping — not because it was an open design question, but simply because `container/` and `parse/` were designed independently and this seam wasn't exercised until `pipeline.rs` actually compiled them together.
7. **Integrating `theme{N}.xml`'s ([Issue #76](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76)) read**: [`parse/theme.md`](parse/theme.en.md) carries the responsibility for resolving the `theme{N}.xml` part's actual path and when to read it forward onto this file. The expected shape mirrors `styles_path`: resolve it from the relationship map in Phase 1 via `THEME_REL_TYPE_SUFFIX = "/relationships/theme"`, and — just like `styles_path` — degrade gracefully to `Workbook::theme = None` when the part itself doesn't exist (following the same `styles_path`/`shared_strings_path` resolution pattern already in the body). But actually delivering the "pay-for-what-you-use" behavior [Issue #76 design proposal](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575) asks for — skipping `theme{N}.xml`'s I/O and parsing entirely when `stylesheet` contains no `ColorRef::Theme` at all — still leaves a choice open: scan the already-built `stylesheet` (built during Phases 1–3) for any `ColorRef::Theme`, or have [`parse/styles.rs`](parse/styles.md) return a flag like `uses_theme_color: bool` alongside building `StyleSheet`. The latter changes `parse/styles.rs`'s return shape and would require a design change on the [styles.md](parse/styles.en.md) side, so this is left to be settled at implementation time.
