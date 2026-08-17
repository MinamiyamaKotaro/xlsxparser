# `src/error.rs` Design Doc

*[日本語](error.md)*

Design doc for `src/error.rs`. Defines the library-wide common error type shared across all 5 phases of the pipeline defined by [architecture.md](architecture.en.md). [model/cell.md](model/cell.en.md) (`CellRef::from_a1`), [model/sheet.md](model/sheet.en.md) (validation of invalid merged ranges), and [model/workbook.md](model/workbook.en.md) (`parse_workbook`'s `Result::Err`) are all written assuming a dependency on the type defined here.

## Responsibility / Scope

- Defines a single crate-wide error enum `Error` (plus `pub type Result<T> = std::result::Result<T, Error>;`)
- Represents the failure modes that can occur in each phase (relationship resolution, sanitization, stream parsing, analysis/deferred resolution) together with the information callers need to handle them (affected path, offending value, etc.)
- Implements `std::error::Error` and holds external crate errors (e.g. `quick-xml`) as `#[source]` so the root cause can be traced via the error chain
- **Not responsible for**: recovery/retry logic (the caller's responsibility), localizing error messages (this library provides only a single set of Rust error strings; i18n, if needed, is left to the caller based on the error variant)

## Key Types (draft)

```rust
use std::path::PathBuf;

/// Crate-wide Result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// The common error type used throughout the library. Every module's failure
/// modes, including `parse_workbook`'s `Result::Err`, are consolidated into
/// this type. Marked `#[non_exhaustive]` so future variants can be added
/// without a breaking change (see Open Question 2).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    // --- Phase 1: relationship resolution ---
    /// A required relationship part, e.g. `xl/_rels/workbook.xml.rels`, is
    /// missing from the ZIP.
    #[error("required relationship part not found: {0}")]
    MissingRelationshipPart(String),

    /// The `r:id` referenced by a `<sheet r:id="...">` element in
    /// `workbook.xml` does not exist in the rels part, or the target file the
    /// rels part points to does not exist in the ZIP.
    #[error("dangling relationship reference: r:id={r_id}")]
    DanglingRelationship { r_id: String },

    // --- Phase 2: sanitization ---
    /// The total uncompressed size exceeded the configured limit (Zip Bomb
    /// protection, requirements spec section 2).
    #[error("zip bomb detected: uncompressed size {actual} bytes exceeds limit {limit} bytes")]
    ZipBombDetected { limit: u64, actual: u64 },

    /// A ZIP entry name contains a path traversal sequence that would escape
    /// the extraction directory (Zip Slip protection).
    #[error("path traversal detected in zip entry: {entry_name}")]
    ZipSlipDetected { entry_name: String },

    /// The ZIP archive itself is corrupt, or a required part of the .xlsx
    /// (OPC) package — e.g. `[Content_Types].xml` or `xl/workbook.xml` — is
    /// missing.
    #[error("not a valid .xlsx package: {0}")]
    InvalidPackage(String),

    // --- Phase 3: stream parsing ---
    /// The XML is syntactically invalid (wraps the underlying quick-xml
    /// parse error).
    #[error("XML parse error in {path}: {source}")]
    XmlParse {
        path: String,
        #[source]
        source: quick_xml::Error,
    },

    /// A required element or attribute is missing from the XML (e.g. a `<c>`
    /// element without an `r` attribute).
    #[error("missing required element/attribute `{name}` in {path}")]
    MissingRequiredElement { path: String, name: &'static str },

    // --- Phase 4: analysis and deferred resolution ---
    /// An A1-style cell reference string is invalid (syntax error, numeric
    /// overflow, empty string, etc. — returned by `CellRef::from_a1` in
    /// model/cell.md).
    #[error("invalid cell reference: {0:?}")]
    InvalidCellRef(String),

    /// A shared string index (referenced via `t="s"`) falls outside the
    /// bounds of the shared string table.
    #[error("shared string index {index} out of bounds (table len={len})")]
    SharedStringIndexOutOfBounds { index: usize, len: usize },

    /// A style ID (index into `cellXfs`) that does not exist was referenced.
    #[error("invalid style id: {0}")]
    InvalidStyleId(u32),

    /// A merged cell range is invalid (overlaps another merged range, or its
    /// start/end coordinates are inverted; used by the validation that
    /// precedes `insert_merge` calls in model/sheet.md).
    #[error("invalid merged cell range {start}:{end}: {reason}")]
    InvalidMergedRange {
        start: String,
        end: String,
        reason: String,
    },

    // --- Common across all phases ---
    /// An I/O error (e.g. the target file cannot be opened or read).
    #[error("I/O error while reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
```

`InvalidPackage` currently serves as a provisional catch-all for ZIP extraction failures (e.g. a corrupt archive). Once `container/`'s design (i.e. which ZIP-handling crate to use) is finalized, revisit whether to split this into a dedicated variant that holds that crate's error type as `#[source]` (see Open Question 1).

## Dependencies

- Depends on: nothing within the crate (the most foundational leaf module — not even `model/` — since `error.rs` depending on any other module would create a cycle). Depends on the external crates `thiserror` (to reduce boilerplate in defining the error type) and `quick-xml` (so `XmlParse::source` can directly hold the XML parser's error type; `quick-xml` has already been settled on as the XML parser in architecture.md).
- Depended on by: nearly every module in the crate (`container/`, `parse/`, `model/`, `resolve/`, `pipeline.rs`, `lib.rs`). `json.rs` only serializes already-resolved data, so it is not expected to generate new instances of this type under normal operation.

`thiserror` is a compile-time-only proc-macro dependency with no impact on runtime binary size or speed, so it does not conflict with the "lightweight and fast" policy in requirements spec section 1.

## Error Handling Policy

- `error.rs` itself performs no error-generating logic (it only defines the type). The following policy applies to everything that uses this type.
- The library never uses `panic!` / `unwrap()` / `expect()` internally. Since the input being parsed is always an untrusted external file, every unexpected input must be propagated to the caller as one of the `Error` variants (the same principle as the error handling policy in [model/cell.md](model/cell.en.md)).
- Errors originating from external crates (e.g. `quick-xml`) are never swallowed; they are held as `#[source]` so the root cause remains traceable via `std::error::Error::source()`.
- Variants carry context (`path`, `r_id`, `index`, etc.) wherever practical, so callers can use it for logging or debugging to identify which file or coordinate the error occurred at.

## Testing Strategy

- Verify that each variant's `Display` (the `#[error(...)]` message) produces the intended string
- Verify that `std::error::Error::source()` correctly returns the root cause for `XmlParse` / `Io`
- Document (rather than test for compilation) that `#[non_exhaustive]` prevents crate consumers from `match`-ing without a `_ =>` arm — i.e. adding a future variant is not a breaking change; this is a compiler-guaranteed language feature, so no separate compile-pass test is needed
- Since this file only defines types, its own unit-test surface is minimal; verification of actual variant construction and propagation is left to the tests of each originating module (e.g. `from_a1` in `model/cell.rs`)

## Open Questions

1. **Which external crate to use for ZIP handling**: to be settled when `container/` is designed. Once chosen, reconsider whether to replace `InvalidPackage(String)` with a dedicated variant holding that crate's error type as `#[source]`, or keep the current `String` catch-all.
2. **Whether to keep `#[non_exhaustive]`**: currently adopted as a general best practice so future variant additions aren't breaking changes. However, while the crate is pre-1.0, adding variants is not considered a breaking change under Cargo's semantic versioning rules in the first place, so this may turn out to be unnecessary until the 1.0 release policy is settled.
3. **Granularity of errors**: currently represented as a single flat enum covering "what happened in which phase," but whether to split it into per-phase nested sub-enums (e.g. `Error::Xml(XmlError)`) if the variant count keeps growing is undecided.
4. **How `InvalidCellRef` / `InvalidMergedRange` hold their input value**: currently designed to hold a `String` (the original input string, or its A1-notation form), but whether they should instead hold the [model/cell.md](model/cell.en.md) `CellRef` type itself (to preserve any successfully-parsed partial information) will be revisited once `CellRef::from_a1`'s failure modes (at what point conversion fails) are finalized during implementation.
5. **MSRV for the `std::error::Error` implementation**: the choice of `thiserror` version (and how it handles `std::error::Error::source()`) depends on the crate's overall MSRV (Minimum Supported Rust Version) policy, which is undecided; to be finalized alongside `Cargo.toml` setup.
