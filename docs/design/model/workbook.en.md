# `model/workbook.rs` Design Doc

*[日本語](workbook.md)*

Design doc for `src/model/workbook.rs`. This is the final data model once resolution processing for all phases has completed, and is exactly what the public API in `lib.rs` (`parse_workbook(path) -> Result<Workbook>`) returns. It aggregates [`Sheet`](sheet.en.md) from model/sheet.md.

## Responsibility / Scope

- Holds multiple [`Sheet`](sheet.en.md) instances in source order (the order defined in `<sheets>` within `xl/workbook.xml`)
- Provides lookup by sheet name
- **Not responsible for**: parsing the `workbook.xml` XML (`parse/workbook.rs`), or resolving the routing between sheet IDs and their backing file paths (`parse/relationships.rs` — the routing map is discarded once Phase 1 completes, so it does not remain in this model)

## Key Types (draft)

```rust
use crate::model::sheet::Sheet;

/// The final resolved output model. The return value of `lib.rs::parse_workbook`.
#[derive(Debug, Clone)]
pub struct Workbook {
    sheets: Vec<Sheet>,
}

impl Workbook {
    /// Builds a `Workbook` from a list of already-resolved sheets.
    /// `pipeline.rs` calls this exactly once, after Phases 3 and 4 have
    /// completed for every sheet (see pipeline.md; added after discovering
    /// the gap while designing it).
    pub(crate) fn new(sheets: Vec<Sheet>) -> Self {
        Self { sheets }
    }

    /// The list of sheets, preserving the definition order from the source file.
    pub fn sheets(&self) -> &[Sheet] {
        &self.sheets
    }

    /// Looks up a sheet by name. Since the requirements spec does not prohibit linear search,
    /// and the number of sheets in practice is small (a few to a few dozen), a linear search
    /// over a Vec is assumed to be sufficient.
    pub fn sheet(&self, name: &str) -> Option<&Sheet> {
        self.sheets.iter().find(|s| s.name == name)
    }
}
```

## Dependencies

- Depends on: [`model/sheet.rs`](sheet.en.md)
- Depended on by: [`pipeline.rs`](../pipeline.en.md) (constructs via `Workbook::new` and returns it), `lib.rs` (the return type of the public API), [`json.rs`](../json.en.md) (the top-level serialization target)

## Error Handling Policy

- `Workbook` itself does not generate errors (it is a terminal model that only holds already-constructed, valid data). Errors during construction (missing required elements in `workbook.xml`, dangling references to sheet backing files, etc.) are the responsibility of `pipeline.rs` / `parse/workbook.rs`, propagated to the caller as `Result::Err` from `parse_workbook` via the common type in `error.rs`.
- `sheet(name)` returns `Option::None` when the sheet does not exist (not a `Result` — a name-lookup miss is a logic error on the caller's side, not an internal library anomaly).

## Testing Strategy

- Looking up `sheet(name)` on a `Workbook` with multiple sheets (both existing and non-existing names)
- Verifying that `sheets()` preserves the source definition order
- Verifying the behavior of `sheets()` / `sheet()` on a `Workbook` with zero sheets (an empty book, or all sheets hidden, etc.)

## Open Questions

1. ~~Handling of hidden sheets~~ → **Resolved**: `Workbook.sheets` includes all sheets regardless of visibility. Excluding hidden sheets could break formula resolution referencing them from other sheets, and would leave the parser incomplete for data-extraction use cases. Visibility is carried as [`Sheet::visibility`](sheet.en.md) (a `SheetVisibility` enum), with filtering left to the caller (or `json.rs`) as an opt-in choice (finalized following the [PR #5 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948235239)).
2. **Complexity of sheet lookup**: For books with an extremely large number of sheets (hundreds, say), a linear search may not scale and a change to something like `IndexMap` could be considered. For now, since the requirements spec's main focus is "grid-paper Excel" (many rows/columns within a single sheet) rather than a large sheet count, Vec + linear search is used as a tentative design.
3. **Book-level metadata**: Information such as author or creation date (the `docProps` family) is out of scope for the requirements spec, but if added in the future, whether to add fields directly to `Workbook` or split them into a separate `Metadata` type is undecided (currently out of scope and not included in the type).
