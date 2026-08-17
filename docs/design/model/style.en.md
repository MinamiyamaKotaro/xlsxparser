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

/// Format information once a style ID has been resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedStyle {
    /// Whether this format represents a date/time. `parse/styles.rs` is
    /// expected to interpret the `numFmts` code string (both built-in and
    /// custom) and store the determination result here ahead of time (see
    /// [resolve/style.md Open Question 2](../resolve/style.en.md)).
    pub is_date_time: bool,
    // Concrete fields for font/fill/border etc. will be finalized when
    // parse/styles.rs is designed (see Open Question 1).
}

/// A table looking up `ResolvedStyle` by `cellXfs` index. Expected to be
/// built by `parse/styles.rs`.
pub type StyleSheet = HashMap<StyleId, Arc<ResolvedStyle>>;
```

## Dependencies

- Depends on: none (a leaf module with no dependency on any sibling module within `model/`, the same position as [`model/cell.rs`](cell.en.md))
- Depended on by: [`model/cell.rs`](cell.en.md) (referenced as `Cell.style: Option<Arc<ResolvedStyle>>`), [`resolve/style.rs`](../resolve/style.en.md) (looks up `StyleSheet` to apply `ResolvedStyle`), `parse/styles.rs` (not yet designed — expected to be the entity that builds `StyleSheet`)

By having both `resolve/` and `parse/` depend only on this file (`model/`) rather than on each other directly, types can be safely handed off between Phase 3 and Phase 4 while preserving [architecture.md](../architecture.en.md) design principle 2 (separation of the I/O layer from domain logic) (addresses PR #8 review feedback).

## Error Handling Policy

Not applicable (as with [`model/cell.rs`](cell.en.md), this file only defines pure, logic-free data structures). Turning a reference to a nonexistent style ID (the case where `StyleSheet::get` returns `None`) into an error is [`resolve/style.rs`](../resolve/style.en.md)'s responsibility.

## Testing Strategy

Not applicable. Since this file contains only type definitions, it has no unit tests. Verification of `ResolvedStyle` equality and `Arc`-sharing behavior is handled by [resolve/style.md](../resolve/style.en.md)'s testing strategy.

## Open Questions

1. **Concrete style elements such as font/fill/border**: the same point as [resolve/style.md Open Question 4](../resolve/style.en.md). `ResolvedStyle` currently only tentatively defines `is_date_time`; how far the requirements spec expects cell styling (font color, background color, borders, bold/italic, etc.) to be included in JSON output will be finalized alongside `json.rs`'s design, or as the requirements spec itself is elaborated.
2. **Where the date/time format determination logic lives**: the same point as [resolve/style.md Open Question 2](../resolve/style.en.md) (undecided). Will be finalized alongside `parse/styles.rs`'s design.
