// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Real RGB values: `Rgb`, `ThemePalette`.
//!
//! Pure data structures shared as vocabulary between `parse/theme.rs`
//! (builds a `ThemePalette` from `theme{N}.xml`) and `resolve/color.rs`
//! (resolves a `model::ColorRef` and `ThemePalette` into a real `Rgb`); this
//! file contains no logic of its own.

/// A real RGB value. A `Copy` type with no heap allocation. Carries no
/// alpha channel — whether a cell's fill is actually visible is controlled
/// by `patternType` (`none`/`solid`/...), not by any notion of color
/// transparency; `ColorRef::Rgb`'s leading two ARGB hex digits are, in
/// practice, always `FF` (opaque), and `resolve::color::resolve_color`
/// simply discards them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// The 12 colors `theme{N}.xml`'s `<clrScheme>` defines, held as a
/// fixed-size array (no heap allocation, Issue #76).
///
/// **Careful**: the array index is *not* the `<clrScheme>` XML declaration
/// order (`dk1, lt1, dk2, lt2, accent1..6, hlink, folHlink`). The index
/// `styles.xml`'s `theme` attribute refers to has slots 0/1 swapped:
/// `lt1, dk1, lt2, dk2, accent1..6, hlink, folHlink` — matching Apache
/// POI's `ThemesTable.ThemeElement` enum, confirmed against real data by a
/// PoC (Issue #76). `parse::theme::parse_theme` is responsible for
/// absorbing this swap when it builds the array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemePalette(pub [Rgb; 12]);
