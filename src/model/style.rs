// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

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

/// A cell fill's foreground/background color, exactly as `<fgColor>`/
/// `<bgColor>` specify it (Issue #75) — kept in its raw, unresolved form
/// rather than converted to a final displayed RGB value. xlsxparser's
/// output is for diffing, not rendering: comparing two `ColorRef`s directly
/// (`PartialEq`) already answers "did this cell's fill color change?"
/// without ever needing to know what color it actually displays as.
/// Resolving `Theme`/`Indexed` to a real RGB value — parsing `theme{N}.xml`,
/// applying the `tint` luminance adjustment, looking up the legacy 64-color
/// palette — is Issue #76's separate concern, needed only by a display use
/// case, not a diff one.
#[derive(Debug, Clone, PartialEq)]
pub enum ColorRef {
    /// `rgb="FFFF0000"` — an 8-hex-digit ARGB string, kept verbatim.
    /// `Arc<str>` for the same reason `number_format` uses it: many
    /// `StyleId`s commonly share the same `fillId` (and thus the same
    /// color), so cloning it per-style should be a refcount bump, not a
    /// fresh heap allocation.
    Rgb(Arc<str>),
    /// `theme="4" tint="-0.25"` — an index into the workbook's
    /// `theme{N}.xml` `<clrScheme>` (12 slots: dk1/lt1/dk2/lt2/accent1-6/
    /// hlink/folHlink), plus an optional luminance adjustment. `tint` is
    /// never present in `theme{N}.xml` itself — it's attached per
    /// reference site (here, in `styles.xml`) — so `None` means "no
    /// `tint` attribute at all" (distinct from an explicit `tint="0"`).
    Theme { index: u32, tint: Option<f64> },
    /// `indexed="64"` — an index into the legacy, pre-OOXML 64-color
    /// palette (64 = "System Foreground", 65 = "System Background" are the
    /// two special values). Real-world writers rarely emit this for a
    /// cell's own fill color today, but it remains part of the schema.
    Indexed(u32),
}

/// Which sides of a cell carry a border (Issue #97) — presence only, not
/// line style/weight or color, the same "not a full transcription" policy
/// `Font` already follows: the grid-paper-detection use case that
/// motivated this only needs to know *whether* a cell is boxed in, not
/// how. `<diagonal>` is not tracked — grid-paper detection has no stated
/// need for it, and Excel's own UI rarely exposes it either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Borders {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl Borders {
    /// Whether any side carries a border at all — `json.rs` uses this to
    /// decide whether to emit a `borders` object at all, the same sparse-
    /// output principle `fill_fg_color`/`col_width_ranges` already follow
    /// (most cells have no border on any side).
    pub fn any(&self) -> bool {
        self.top || self.right || self.bottom || self.left
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
    /// Whether this format represents a date/time. `parse/styles.rs`
    /// interprets the `numFmts` code string (built-in and custom) and stores
    /// the determination result here ahead of time.
    pub is_date_time: bool,
    pub font: Font,
    /// `<cellXfs><xf><alignment wrapText="1"/></xf></cellXfs>` (Issue #37)
    /// — the downstream grid-paper detector's overflow-detection heuristic
    /// uses this as a gate: a wrapped cell is never flagged as overflowing.
    pub wrap_text: bool,
    /// `<cellXfs><xf><alignment horizontal=".."/></xf></cellXfs>` (Issue
    /// #42). Named `horizontal_` (rather than plain `alignment`) so a
    /// future `vertical_alignment` field doesn't collide with it or need a
    /// rename.
    pub horizontal_alignment: Alignment,
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
    /// `<fill><patternFill><fgColor .../></patternFill></fill>` (Issue
    /// #75), raw/unresolved — see [`ColorRef`]. `None` when the `<fill>`
    /// has no `<fgColor>` at all (e.g. `patternType="none"`/`"gray125"`
    /// with no color children), not just when `fillId` is absent/0.
    pub fill_fg_color: Option<ColorRef>,
    /// Same as `fill_fg_color`, for `<bgColor>`. For a `solid` pattern fill
    /// (the only pattern this library's callers have needed so far),
    /// `fgColor` is what Excel actually paints the cell with — `bgColor` is
    /// a secondary layer only visible through non-solid hatching patterns
    /// — but both are kept since either can independently change between
    /// two versions of a file, which is what a diff cares about.
    pub fill_bg_color: Option<ColorRef>,
    /// `<xf borderId="..">` resolved against `<borders>` (Issue #97),
    /// presence-only per side. Nested (like `font: Font`) rather than
    /// flattened into four top-level fields — `Borders`'s four booleans
    /// naturally group as one concept the way `Font`'s `size_pt`/`bold`
    /// do, unlike `fill_fg_color`/`fill_bg_color`'s split (which exists so
    /// each can be diffed as an independent `Option<ColorRef>`).
    pub borders: Borders,
}

/// A table looking up `ResolvedStyle` by `cellXfs` index. Built by
/// `parse/styles.rs`, applied by `resolve/style.rs`.
pub type StyleSheet = HashMap<StyleId, Arc<ResolvedStyle>>;
