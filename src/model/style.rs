//! Resolved cell style information: `StyleId`, `ResolvedStyle`, `StyleSheet`.
//!
//! Pure data structures shared as vocabulary between `parse/styles.rs`
//! (builds a `StyleSheet` from `styles.xml`) and `resolve/style.rs` (applies
//! a `ResolvedStyle` to cells); this file contains no logic of its own.

use std::collections::HashMap;
use std::sync::Arc;

/// The `cellXfs` index (style ID). Kept type-consistent with
/// `Error::InvalidStyleId(u32)`.
pub type StyleId = u32;

/// A resolved `<font>` entry: just the two properties Issue #38 needs
/// (`size_pt`/`bold` feed the downstream grid-paper detector's overflow-
/// width estimate and heading-block heuristic), not a full transcription
/// of `CT_Font` (color, name, italic, underline, ... are out of scope
/// until a use case needs them).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Font {
    pub size_pt: f64,
    pub bold: bool,
}

impl Default for Font {
    /// Excel's own default (`Normal` style, "Calibri 11", not bold) — used
    /// when an `<xf>`'s `fontId` is absent or does not resolve to a parsed
    /// `<font>` entry (`parse/styles.rs`'s graceful-degradation policy,
    /// matching how a missing/unresolvable `numFmtId` falls back to "not a
    /// date" rather than erroring).
    fn default() -> Self {
        Font {
            size_pt: 11.0,
            bold: false,
        }
    }
}

/// Format information once a style ID has been resolved.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolvedStyle {
    /// Whether this format represents a date/time. `parse/styles.rs`
    /// interprets the `numFmts` code string (built-in and custom) and stores
    /// the determination result here ahead of time.
    pub is_date_time: bool,
    pub font: Font,
    /// `<cellXfs><xf><alignment wrapText="1"/></xf></cellXfs>` (Issue #37)
    /// — the downstream grid-paper detector's overflow-detection heuristic
    /// uses this as a gate: a wrapped cell is never flagged as overflowing.
    pub wrap_text: bool,
    /// The `numFmtId` referenced by `<xf>`, resolved to its format-code
    /// string (Issue #41) — built-in (ECMA-376 Part 1 §18.8.30) or custom
    /// (`<numFmts>`). `None` represents `numFmtId=0` ("General"), an
    /// absent `numFmtId` attribute, or a reference to neither a known
    /// built-in nor a defined custom format — "General" carries no
    /// information a downstream consumer needs beyond "no special format",
    /// so it is treated the same as "nothing to report" rather than
    /// `Some("General")`. `Arc<str>` avoids duplicating the format-code
    /// string across every `StyleId` that shares the same `numFmtId`, the
    /// same reasoning `CellValue::Text` already applies.
    pub number_format: Option<Arc<str>>,
    // Concrete fields for fill/border/other alignment properties etc. are
    // added as their own sub-issues land (see
    // docs/design/model/style.en.md Open Question 1).
}

/// A table looking up `ResolvedStyle` by `cellXfs` index. Built by
/// `parse/styles.rs`, applied by `resolve/style.rs`.
pub type StyleSheet = HashMap<StyleId, Arc<ResolvedStyle>>;
