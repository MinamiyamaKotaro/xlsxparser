# `model/workbook.rs` Design Doc

*[日本語](workbook.md)*

Design doc for `src/model/workbook.rs`. This is the final data model once resolution processing for all phases has completed, and is exactly what the public API in `lib.rs` (`parse_workbook(path) -> Result<Workbook>`) returns. It aggregates [`Sheet`](sheet.en.md) from model/sheet.md.

## Responsibility / Scope

- Holds multiple [`Sheet`](sheet.en.md) instances in source order (the order defined in `<sheets>` within `xl/workbook.xml`)
- Provides lookup by sheet name
- If the workbook has a `theme{N}.xml` part, holds its [`ThemePalette`](color.en.md) ([Issue #76](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76) — see Open Question 4 below)
- **Not responsible for**: parsing the `workbook.xml` XML (`parse/workbook.rs`), or resolving the routing between sheet IDs and their backing file paths (`parse/relationships.rs` — the routing map is discarded once Phase 1 completes, so it does not remain in this model), XML parsing of `theme{N}.xml` itself ([`parse/theme.rs`](../parse/theme.en.md)), the logic that resolves a `ColorRef` to a real RGB value ([`resolve/color.rs`](../resolve/color.en.md) — this file only holds the already-built `ThemePalette` and lends it out to callers)

## Key Types (draft)

```rust
use crate::model::color::ThemePalette;
use crate::model::sheet::Sheet;

/// The final resolved output model. The return value of `lib.rs::parse_workbook`.
#[derive(Debug, Clone)]
pub struct Workbook {
    sheets: Vec<Sheet>,
    /// The 12-color theme palette resolved from the `xl/theme/theme{N}.xml`
    /// part. `None` for a workbook without the part at all (the vast
    /// majority of files, which never use theme colors — Issue #76).
    /// Passing this to [`resolve::color::resolve_color`](../resolve/color.en.md)
    /// resolves a `ColorRef::Theme` on `ResolvedStyle.fill_fg_color` and
    /// similar fields to a real RGB value — the "Option A: on-demand
    /// resolve API"
    /// ([Issue #76 design proposal](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575))
    /// only works if callers can reach a `ThemePalette` paired with a
    /// `ColorRef`, so this field was added during this design pass to
    /// provide that path (see [model/color.md Open Question 2](color.en.md)).
    theme: Option<ThemePalette>,
}

impl Workbook {
    /// Builds a `Workbook` from a list of already-resolved sheets and a
    /// theme palette. `pipeline.rs` calls this exactly once, after Phases
    /// 3 and 4 have completed for every sheet (see pipeline.md; added
    /// after discovering the gap while designing it).
    pub(crate) fn new(sheets: Vec<Sheet>, theme: Option<ThemePalette>) -> Self {
        Self { sheets, theme }
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

    /// The theme palette, if present. Meant to be passed straight through
    /// as the `theme` argument to
    /// [`resolve::color::resolve_color`](../resolve/color.en.md) when
    /// resolving a `ColorRef::Theme` to a real RGB value.
    pub fn theme(&self) -> Option<&ThemePalette> {
        self.theme.as_ref()
    }
}
```

## Dependencies

- Depends on: [`model/sheet.rs`](sheet.en.md), [`model/color.rs`](color.en.md) (`ThemePalette`. Issue #76)
- Depended on by: [`pipeline.rs`](../pipeline.en.md) (constructs via `Workbook::new` and returns it — reading the `theme{N}.xml` part and calling [`parse/theme.rs`](../parse/theme.en.md) is tracked at [pipeline.md Open Question 6](../pipeline.en.md)), `lib.rs` (the return type of the public API), [`json.rs`](../json.en.md) (the top-level serialization target; does not include `theme` in JSON output today — see [resolve/color.md Open Question 1](../resolve/color.en.md)), external callers outside the crate (combine `Workbook::theme()` with `ResolvedStyle.fill_fg_color`/`fill_bg_color` and pass both to [`resolve::color::resolve_color`](../resolve/color.en.md) to get a real RGB value — the display use case Issue #76 targets)

## Error Handling Policy

- `Workbook` itself does not generate errors (it is a terminal model that only holds already-constructed, valid data). Errors during construction (missing required elements in `workbook.xml`, dangling references to sheet backing files, etc.) are the responsibility of `pipeline.rs` / `parse/workbook.rs`, propagated to the caller as `Result::Err` from `parse_workbook` via the common type in `error.rs`.
- `sheet(name)` returns `Option::None` when the sheet does not exist (not a `Result` — a name-lookup miss is a logic error on the caller's side, not an internal library anomaly).

## Testing Strategy

- Looking up `sheet(name)` on a `Workbook` with multiple sheets (both existing and non-existing names)
- Verifying that `sheets()` preserves the source definition order
- Verifying the behavior of `sheets()` / `sheet()` on a `Workbook` with zero sheets (an empty book, or all sheets hidden, etc.)
- Verifying `theme()` returns `Some(&ThemePalette)` for a `Workbook` with a `theme{N}.xml` part, and `None` for one without (Issue #76)

## Open Questions

1. ~~Handling of hidden sheets~~ → **Resolved**: `Workbook.sheets` includes all sheets regardless of visibility. Excluding hidden sheets could break formula resolution referencing them from other sheets, and would leave the parser incomplete for data-extraction use cases. Visibility is carried as [`Sheet::visibility`](sheet.en.md) (a `SheetVisibility` enum), with filtering left to the caller (or `json.rs`) as an opt-in choice (finalized following the [PR #5 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948235239)).
2. **Complexity of sheet lookup**: For books with an extremely large number of sheets (hundreds, say), a linear search may not scale and a change to something like `IndexMap` could be considered. For now, since the requirements spec's main focus is "grid-paper Excel" (many rows/columns within a single sheet) rather than a large sheet count, Vec + linear search is used as a tentative design.
3. **Book-level metadata**: Information such as author or creation date (the `docProps` family) is out of scope for the requirements spec, but if added in the future, whether to add fields directly to `Workbook` or split them into a separate `Metadata` type is undecided (currently out of scope and not included in the type).
4. **The `theme` field is a gap-filling addition made during this design pass**: [Issue #76 design proposal](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575) itself never mentions a change to `Workbook` — it turned out to be necessary, while writing this design, to actually make [`resolve/color.md`](../resolve/color.en.md)'s "Option A" callable. How and when `theme{N}.xml`'s actual path gets resolved and read (a `pipeline.rs`-side change) remains open as [pipeline.md Open Question 6](../pipeline.en.md); `Workbook::new`'s exact signature will be finalized alongside that at implementation time.
