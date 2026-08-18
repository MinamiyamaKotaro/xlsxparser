# `model/style.rs` Design Doc

*[日本語](style.md)*

Design doc for `src/model/style.rs`. Newly added to resolve where `ResolvedStyle` should live, a question that [model/mod.md Open Question 1](mod.en.md) and [model/cell.md](cell.en.md) both left as "undecided whether to place it in `model/` or on the `resolve/style.rs` side" (addresses PR #8 review feedback). Defines only pure, logic-free data structures representing resolved cell style information.

This file serves as the shared vocabulary between Phase 3 and Phase 4: `parse/styles.rs` (not yet designed — the entity that builds `ResolvedStyle` from `styles.xml`) and [`resolve/style.rs`](../resolve/style.en.md) (the entity that applies an already-built `ResolvedStyle` to cells) are connected indirectly, only through the types defined here, without knowing about each other directly. This is the same role that [`model/cell.rs`](cell.en.md)'s `Cell` / `Sheet` play as shared data structures referenced by both `parse/` and `resolve/`.

## Responsibility / Scope

- Defines `StyleId`, the type for a `cellXfs` index (style ID)
- Defines `ResolvedStyle`, the format information once a style ID has been resolved
- Defines `StyleSheet`, a table type that looks up `ResolvedStyle` by `cellXfs` index
- **Not responsible for**: XML parsing of `styles.xml` or the logic that builds `ResolvedStyle` itself (`parse/styles.rs`, not yet designed); the process of applying `ResolvedStyle` to cells itself ([`resolve/style.rs`](../resolve/style.en.md)); the concrete implementation of the numFmt code rules that determine whether a format is a date/time format (see [resolve/style.md Open Question 2](../resolve/style.en.md) — this file only defines the field that holds the determination result)

## Key Types (draft)

```rust
use std::collections::HashMap;
use std::sync::Arc;

/// The `cellXfs` index (style ID). Kept type-consistent with
/// [error.rs](../error.en.md)'s `Error::InvalidStyleId(u32)`.
pub type StyleId = u32;

/// A resolved `<font>` entry: just the two properties Issue #38 needs, not
/// a full transcription of `CT_Font` (color, name, italic, underline, ...
/// stay out of scope until a use case needs them — see Open Question 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Font {
    pub size_pt: f64,
    pub bold: bool,
}

impl Default for Font {
    /// Excel's own default ("Normal" style, "Calibri 11", not bold) — the
    /// fallback `parse/styles.rs` uses when an `<xf>`'s `fontId` is absent
    /// or unresolvable, matching the graceful-degradation policy
    /// `is_date_time` already established for a missing/invalid `numFmtId`.
    fn default() -> Self {
        Font { size_pt: 11.0, bold: false }
    }
}

/// `<xf><alignment horizontal=".."/></xf>`'s horizontal alignment (ECMA-376
/// `ST_HorizontalAlignmentValues`), Issue #42. An `enum` rather than a
/// string so it stays a cheap `Copy` value (Issue #42's stated performance
/// requirement) — vertical alignment and every other `CT_CellAlignment`
/// attribute besides `wrapText`/`horizontal` stay out of scope until a
/// concrete downstream use case needs them, the same "not a full
/// transcription" policy `Font` already follows.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Alignment {
    #[default]
    General,
    Left,
    Center,
    Right,
    Fill,
    Justify,
    CenterContinuous,
    Distributed,
}

/// Format information once a style ID has been resolved.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolvedStyle {
    /// Whether this format represents a date/time. `parse/styles.rs` is
    /// expected to interpret the `numFmts` code string (both built-in and
    /// custom) and store the determination result here ahead of time (see
    /// [resolve/style.md Open Question 2](../resolve/style.en.md)).
    pub is_date_time: bool,
    pub font: Font,
    /// `<cellXfs><xf><alignment wrapText="1"/></xf></cellXfs>` (Issue #37)
    /// — the downstream grid-paper detector gates its overflow heuristic
    /// on this: a wrapped cell is never flagged as overflowing.
    pub wrap_text: bool,
    /// `<cellXfs><xf><alignment horizontal=".."/></xf></cellXfs>` (Issue
    /// #42). Named `horizontal_` (rather than plain `alignment`) so a
    /// future `vertical_alignment` field doesn't collide with it or need a
    /// rename.
    pub horizontal_alignment: Alignment,
    /// The `numFmtId` referenced by `<xf>`, resolved to its format-code
    /// string (Issue #41) — built-in (ECMA-376 Part 1 §18.8.30) or custom
    /// (`<numFmts>`). `None` for `numFmtId=0` ("General"), an absent
    /// `numFmtId`, or an ID resolving to neither table — "General" carries
    /// no information beyond "nothing special", so it gets the same
    /// treatment as "not found" rather than `Some("General")`. `Arc<str>`
    /// for the same reason `CellValue::Text` uses it: the same format code
    /// is frequently shared across many `StyleId`s.
    pub number_format: Option<Arc<str>>,
    // Concrete fields for fill/border etc. will be added as their own
    // sub-issues land (see Open Question 1).
}

/// A table looking up `ResolvedStyle` by `cellXfs` index. Expected to be
/// built by `parse/styles.rs`.
pub type StyleSheet = HashMap<StyleId, Arc<ResolvedStyle>>;
```

`ResolvedStyle`/`Font` both derive/implement `Default` so call sites needing only a subset of fields (most test fixtures) can write `ResolvedStyle { is_date_time: true, ..Default::default() }` rather than naming every field.

## Dependencies

- Depends on: none (a leaf module with no dependency on any sibling module within `model/`, the same position as [`model/cell.rs`](cell.en.md))
- Depended on by: [`model/cell.rs`](cell.en.md) (referenced as `Cell.style: Option<Arc<ResolvedStyle>>`), [`resolve/style.rs`](../resolve/style.en.md) (looks up `StyleSheet` to apply `ResolvedStyle`), `parse/styles.rs` (not yet designed — expected to be the entity that builds `StyleSheet`)

By having both `resolve/` and `parse/` depend only on this file (`model/`) rather than on each other directly, types can be safely handed off between Phase 3 and Phase 4 while preserving [architecture.md](../architecture.en.md) design principle 2 (separation of the I/O layer from domain logic) (addresses PR #8 review feedback).

## Error Handling Policy

Not applicable (as with [`model/cell.rs`](cell.en.md), this file only defines pure, logic-free data structures). Turning a reference to a nonexistent style ID (the case where `StyleSheet::get` returns `None`) into an error is [`resolve/style.rs`](../resolve/style.en.md)'s responsibility.

## Testing Strategy

Not applicable. Since this file contains only type definitions, it has no unit tests. Verification of `ResolvedStyle` equality and `Arc`-sharing behavior is handled by [resolve/style.md](../resolve/style.en.md)'s testing strategy.

## Open Questions

1. **Concrete style elements such as fill/border/wrap/alignment**: further resolved — `font: Font { size_pt, bold }` (Issue #38), `wrap_text: bool` (Issue #37, the overflow-heuristic gate), `number_format: Option<Arc<str>>` (Issue #41), and `horizontal_alignment: Alignment` (Issue #42) are all implemented. Every sub-issue under [Issue #36](https://github.com/MinamiyamaKotaro/xlsxparser/issues/36) is now resolved. Font color, fill, border, italic, underline, and any other `CT_Font`/`CT_Fill`/`CT_Border` properties, plus every other `CT_CellAlignment` attribute besides `wrapText`/`horizontal` (vertical alignment, indent, text rotation, ...), remain out of scope until a concrete downstream use case names them — the same "not a full transcription" policy `Font` already follows.
2. ~~Where the date/time format determination logic lives~~ → **Resolved**: [`parse/styles.rs`](../parse/styles.en.md) owns classifying `ResolvedStyle::is_date_time` from `numFmtId`/`formatCode` (the same point as [resolve/style.md Open Question 2](../resolve/style.en.md)). The heuristic's precision itself remains open — see [parse/styles.md Open Question 2](../parse/styles.en.md).
