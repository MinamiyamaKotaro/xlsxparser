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

/// Format information once a style ID has been resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedStyle {
    /// Whether this format represents a date/time. `parse/styles.rs`
    /// interprets the `numFmts` code string (built-in and custom) and stores
    /// the determination result here ahead of time.
    pub is_date_time: bool,
    // Concrete fields for font/fill/border etc. are added once
    // parse/styles.rs is implemented (see docs/design/model/style.en.md
    // Open Question 1).
}

/// A table looking up `ResolvedStyle` by `cellXfs` index. Built by
/// `parse/styles.rs`, applied by `resolve/style.rs`.
pub type StyleSheet = HashMap<StyleId, Arc<ResolvedStyle>>;
