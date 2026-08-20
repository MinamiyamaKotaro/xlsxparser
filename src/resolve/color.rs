// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Resolves a `model::ColorRef` (theme/indexed/rgb reference) to a real
//! `model::Rgb` value (Issue #76). Pure functions, no I/O — never called
//! from `resolve_sheet`'s per-cell pipeline; this is an on-demand API a
//! display-oriented caller invokes only where it actually needs a color
//! (see docs/design/resolve/color.en.md).

use crate::model::{ColorRef, Rgb, ThemePalette};

/// ECMA-376's legacy fixed 64-color palette (`indexed=0..=63`). A
/// compile-time constant array, zero runtime memory cost. Confirmed by a
/// PoC to match both a bundled fixture's own `<colors><indexedColors>`
/// re-declaration and `openpyxl.styles.colors.COLOR_INDEX` (Issue #76).
const fn rgb(hex: u32) -> Rgb {
    Rgb {
        r: ((hex >> 16) & 0xFF) as u8,
        g: ((hex >> 8) & 0xFF) as u8,
        b: (hex & 0xFF) as u8,
    }
}

const INDEXED_PALETTE: [Rgb; 64] = [
    rgb(0x000000),
    rgb(0xFFFFFF),
    rgb(0xFF0000),
    rgb(0x00FF00),
    rgb(0x0000FF),
    rgb(0xFFFF00),
    rgb(0xFF00FF),
    rgb(0x00FFFF),
    rgb(0x000000),
    rgb(0xFFFFFF),
    rgb(0xFF0000),
    rgb(0x00FF00),
    rgb(0x0000FF),
    rgb(0xFFFF00),
    rgb(0xFF00FF),
    rgb(0x00FFFF),
    rgb(0x800000),
    rgb(0x008000),
    rgb(0x000080),
    rgb(0x808000),
    rgb(0x800080),
    rgb(0x008080),
    rgb(0xC0C0C0),
    rgb(0x808080),
    rgb(0x9999FF),
    rgb(0x993366),
    rgb(0xFFFFCC),
    rgb(0xCCFFFF),
    rgb(0x660066),
    rgb(0xFF8080),
    rgb(0x0066CC),
    rgb(0xCCCCFF),
    rgb(0x000080),
    rgb(0xFF00FF),
    rgb(0xFFFF00),
    rgb(0x00FFFF),
    rgb(0x800080),
    rgb(0x800000),
    rgb(0x008080),
    rgb(0x0000FF),
    rgb(0x00CCFF),
    rgb(0xCCFFFF),
    rgb(0xCCFFCC),
    rgb(0xFFFF99),
    rgb(0x99CCFF),
    rgb(0xFF99CC),
    rgb(0xCC99FF),
    rgb(0xFFCC99),
    rgb(0x3366FF),
    rgb(0x33CCCC),
    rgb(0x99CC00),
    rgb(0xFFCC00),
    rgb(0xFF9900),
    rgb(0xFF6600),
    rgb(0x666699),
    rgb(0x969696),
    rgb(0x003366),
    rgb(0x339966),
    rgb(0x003300),
    rgb(0x333300),
    rgb(0x993300),
    rgb(0x993366),
    rgb(0x333399),
    rgb(0x333333),
];

/// Applies `tint` luminance correction to a base color from
/// `theme{N}.xml`'s `<clrScheme>`. Returns `base` unchanged when `tint` is
/// `0.0` or non-finite (`NaN`/`Inf`) — a safe degradation against a crafted
/// `tint` value.
///
/// Formula (ECMA-376's luminance-correction algorithm, confirmed against
/// Apache POI's implementation and an independent Python `colorsys`
/// re-implementation during design PoC — Issue #76): when `tint > 0`,
/// `l' = l*(1-tint) + tint` (lighten); when `tint < 0`, `l' = l*(1+tint)`
/// (darken).
pub(crate) fn apply_tint(base: Rgb, tint: f64) -> Rgb {
    if !tint.is_finite() || tint == 0.0 {
        return base;
    }
    let (h, s, l) = rgb_to_hsl(base);
    let l2 = if tint > 0.0 {
        l * (1.0 - tint) + tint
    } else {
        l * (1.0 + tint)
    };
    hsl_to_rgb(h, s, l2.clamp(0.0, 1.0))
}

fn rgb_to_hsl(c: Rgb) -> (f64, f64, f64) {
    let r = c.r as f64 / 255.0;
    let g = c.g as f64 / 255.0;
    let b = c.b as f64 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < f64::EPSILON {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn hue_to_rgb(p: f64, q: f64, mut t: f64) -> f64 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 1.0 / 2.0 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> Rgb {
    if s.abs() < f64::EPSILON {
        let v = (l * 255.0).round() as u8;
        return Rgb { r: v, g: v, b: v };
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let r = hue_to_rgb(p, q, h + 1.0 / 3.0);
    let g = hue_to_rgb(p, q, h);
    let b = hue_to_rgb(p, q, h - 1.0 / 3.0);
    Rgb {
        r: (r * 255.0).round() as u8,
        g: (g * 255.0).round() as u8,
        b: (b * 255.0).round() as u8,
    }
}

/// Resolves a legacy indexed color (the `indexed` attribute) to a real RGB
/// value. `0..=63` looks up [`INDEXED_PALETTE`]. `64`/`65` are special
/// values for the system foreground/background colors, resolved as
/// OS-independent, deterministic fixed colors (`64→black`, `65→white` —
/// this crate runs headless and cannot depend on an OS system palette).
/// `66` and above are out of range and return `None` rather than
/// panicking.
pub(crate) fn lookup_indexed_color(index: u32) -> Option<Rgb> {
    match index {
        0..=63 => Some(INDEXED_PALETTE[index as usize]),
        64 => Some(Rgb {
            r: 0x00,
            g: 0x00,
            b: 0x00,
        }),
        65 => Some(Rgb {
            r: 0xFF,
            g: 0xFF,
            b: 0xFF,
        }),
        _ => None,
    }
}

/// Parses an 8-digit ARGB hex string (`"FFFF0000"`, ECMA-376's
/// `ST_UnsignedIntHex` — the only form `<fgColor rgb="..">` ever legally
/// takes) into an `Rgb`, discarding the two alpha digits. `ColorRef::Rgb`
/// never validates its value at parse time and keeps it verbatim, so this
/// returns `None` for anything that isn't exactly 8 valid hex digits,
/// rather than panicking.
///
/// Parses the *entire* 8-digit string as one `u32` — rather than slicing
/// off the leading 2 alpha digits and parsing only the remaining 6 —
/// for two reasons: it validates the alpha digits are hex too (a value
/// like `"GGFF0000"` must be rejected as malformed, not silently accepted
/// just because the RGB tail happens to parse), and it avoids any string
/// slicing at all, sidestepping the char-boundary panic risk `&s[2..]`
/// would otherwise carry (`len()` counts bytes, not chars, so an 8-*byte*
/// string containing multi-byte UTF-8 can have byte index 2 land
/// mid-codepoint). The alpha byte is simply never read out of `v`.
fn parse_argb_hex(s: &str) -> Option<Rgb> {
    if s.len() != 8 || !s.is_ascii() {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(Rgb {
        r: ((v >> 16) & 0xFF) as u8,
        g: ((v >> 8) & 0xFF) as u8,
        b: (v & 0xFF) as u8,
    })
}

/// Resolves the real RGB value a `ColorRef` refers to. `theme` is `Some`
/// only when the workbook has a `theme{N}.xml` part
/// (`model::Workbook::theme`; `None` for a workbook without the part at
/// all).
///
/// No branch ever panics — invalid or crafted input (an out-of-range
/// `theme` index, malformed hex, an out-of-range `indexed` value) degrades
/// safely to `None`.
pub fn resolve_color(color: &ColorRef, theme: Option<&ThemePalette>) -> Option<Rgb> {
    match color {
        ColorRef::Rgb(s) => parse_argb_hex(s),
        ColorRef::Theme { index, tint } => {
            let palette = theme?;
            let base = *palette.0.get(*index as usize)?;
            Some(match tint {
                Some(t) => apply_tint(base, *t),
                None => base,
            })
        }
        ColorRef::Indexed(index) => lookup_indexed_color(*index),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn rgb_val(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    #[test]
    fn apply_tint_zero_is_unchanged() {
        let base = rgb_val(0x4F, 0x81, 0xBD);
        assert_eq!(apply_tint(base, 0.0), base);
    }

    #[test]
    fn apply_tint_non_finite_is_unchanged_and_does_not_panic() {
        let base = rgb_val(0x4F, 0x81, 0xBD);
        assert_eq!(apply_tint(base, f64::NAN), base);
        assert_eq!(apply_tint(base, f64::INFINITY), base);
        assert_eq!(apply_tint(base, f64::NEG_INFINITY), base);
    }

    #[test]
    fn apply_tint_boundary_values_converge_to_white_and_black() {
        let base = rgb_val(0x4F, 0x81, 0xBD);
        assert_eq!(apply_tint(base, 1.0), rgb_val(0xFF, 0xFF, 0xFF));
        assert_eq!(apply_tint(base, -1.0), rgb_val(0x00, 0x00, 0x00));
    }

    #[test]
    fn apply_tint_matches_poc_verified_value() {
        // PoC-verified against an independent Python `colorsys`
        // re-implementation (Issue #76 comment #5352366260):
        // accent1 (#4F81BD) with tint -0.25 -> #376092.
        let base = rgb_val(0x4F, 0x81, 0xBD);
        assert_eq!(apply_tint(base, -0.25), rgb_val(0x37, 0x60, 0x92));
    }

    // The 5 cases below exercise `rgb_to_hsl`/`hue_to_rgb`'s remaining hue
    // octants and the achromatic (gray) path that #4F81BD alone never
    // reaches — each independently cross-checked against Python's
    // `colorsys` (the same independent-implementation verification method
    // Issue #76's design PoC used), not just chosen to pad coverage.
    #[test]
    fn apply_tint_pure_red_max_channel_is_r() {
        // tint=-0.33 rather than a round-numbered value like -0.3: the
        // latter happens to land the intermediate lightness exactly on a
        // 178.5 rounding boundary, where Rust's f64::round() (round half
        // away from zero) and Python's round() (round half to even) —
        // otherwise in full agreement everywhere else this was
        // cross-checked — legitimately disagree by 1 ULP-of-a-color-value.
        // Both are "correct" by their own convention; -0.33 just avoids
        // asserting a value that depends on which convention is used.
        assert_eq!(
            apply_tint(rgb_val(0xFF, 0x00, 0x00), -0.33),
            rgb_val(0xAB, 0x00, 0x00)
        );
    }

    #[test]
    fn apply_tint_pure_green_max_channel_is_g() {
        assert_eq!(
            apply_tint(rgb_val(0x00, 0xFF, 0x00), 0.4),
            rgb_val(0x66, 0xFF, 0x66)
        );
    }

    #[test]
    fn apply_tint_achromatic_gray_stays_gray() {
        // s == 0: exercises rgb_to_hsl's early achromatic return and
        // hsl_to_rgb's s.abs() < EPSILON fast path, neither of which
        // #4F81BD (a chromatic color) ever reaches.
        let result = apply_tint(rgb_val(0x80, 0x80, 0x80), 0.5);
        assert_eq!(result, rgb_val(0xC0, 0xC0, 0xC0));
        assert_eq!(result.r, result.g);
        assert_eq!(result.g, result.b);
    }

    #[test]
    fn apply_tint_orange_max_channel_is_r_with_g_at_least_b() {
        assert_eq!(
            apply_tint(rgb_val(0xFF, 0x80, 0x00), -0.2),
            rgb_val(0xCC, 0x66, 0x00)
        );
    }

    #[test]
    fn apply_tint_pink_max_channel_is_r_with_g_less_than_b() {
        assert_eq!(
            apply_tint(rgb_val(0xFF, 0x00, 0x80), 0.2),
            rgb_val(0xFF, 0x33, 0x99)
        );
    }

    #[test]
    fn rgb_const_fn_decomposes_a_hex_literal() {
        // INDEXED_PALETTE builds entirely at compile time via `rgb()`, so
        // no test exercises it at runtime otherwise; this both closes that
        // coverage gap and is a direct unit test of the helper itself.
        assert_eq!(rgb(0x4F81BD), rgb_val(0x4F, 0x81, 0xBD));
        assert_eq!(rgb(0x000000), rgb_val(0x00, 0x00, 0x00));
        assert_eq!(rgb(0xFFFFFF), rgb_val(0xFF, 0xFF, 0xFF));
    }

    #[test]
    fn lookup_indexed_color_covers_the_table_and_boundaries() {
        assert_eq!(lookup_indexed_color(0), Some(rgb_val(0x00, 0x00, 0x00)));
        assert_eq!(lookup_indexed_color(63), Some(rgb_val(0x33, 0x33, 0x33)));
        assert_eq!(lookup_indexed_color(64), Some(rgb_val(0x00, 0x00, 0x00)));
        assert_eq!(lookup_indexed_color(65), Some(rgb_val(0xFF, 0xFF, 0xFF)));
        assert_eq!(lookup_indexed_color(66), None);
        assert_eq!(lookup_indexed_color(u32::MAX), None);
    }

    #[test]
    fn resolve_color_rgb_variant() {
        let color = ColorRef::Rgb(Arc::from("FFFF0000"));
        assert_eq!(resolve_color(&color, None), Some(rgb_val(0xFF, 0x00, 0x00)));
    }

    #[test]
    fn resolve_color_rgb_variant_invalid_hex_is_none() {
        let color = ColorRef::Rgb(Arc::from("not-a-color"));
        assert_eq!(resolve_color(&color, None), None);
    }

    #[test]
    fn resolve_color_rgb_variant_invalid_alpha_digits_is_none() {
        // The alpha digits are discarded from the *returned* Rgb, but they
        // must still be validated as hex — "GGFF0000" must not silently
        // resolve just because its RGB tail happens to parse.
        let color = ColorRef::Rgb(Arc::from("GGFF0000"));
        assert_eq!(resolve_color(&color, None), None);
    }

    #[test]
    fn resolve_color_rgb_variant_multibyte_utf8_at_the_expected_byte_length_does_not_panic() {
        // Regression test: "a" (1 byte) + "€" (3 bytes, U+20AC) + "1234"
        // (4 bytes) totals 8 *bytes* (matching the ARGB length check), but
        // byte index 2 falls inside "€"'s 3-byte encoding — not a char
        // boundary. An earlier version sliced `&s[2..]` before checking
        // `is_ascii()`, which panicked on this input ("byte index 2 is not
        // a char boundary"); the current implementation never slices `s`
        // at all. ColorRef::Rgb never validates its value at parse time,
        // so this exact shape is reachable from an untrusted, crafted
        // .xlsx file.
        let multibyte = "a€1234";
        assert_eq!(multibyte.len(), 8);
        let color = ColorRef::Rgb(Arc::from(multibyte));
        assert_eq!(resolve_color(&color, None), None);
    }

    fn office_palette() -> ThemePalette {
        ThemePalette([
            rgb_val(0xFF, 0xFF, 0xFF), // 0: lt1
            rgb_val(0x00, 0x00, 0x00), // 1: dk1
            rgb_val(0xEE, 0xEC, 0xE1), // 2: lt2
            rgb_val(0x1F, 0x49, 0x7D), // 3: dk2
            rgb_val(0x4F, 0x81, 0xBD), // 4: accent1
            rgb_val(0xC0, 0x50, 0x4D), // 5: accent2
            rgb_val(0x9B, 0xBB, 0x59), // 6: accent3
            rgb_val(0x80, 0x64, 0xA2), // 7: accent4
            rgb_val(0x4B, 0xAC, 0xC6), // 8: accent5
            rgb_val(0xF7, 0x96, 0x46), // 9: accent6
            rgb_val(0x00, 0x00, 0xFF), // 10: hlink
            rgb_val(0x80, 0x00, 0x80), // 11: folHlink
        ])
    }

    #[test]
    fn resolve_color_theme_variant_with_tint() {
        let color = ColorRef::Theme {
            index: 4,
            tint: Some(-0.25),
        };
        let palette = office_palette();
        assert_eq!(
            resolve_color(&color, Some(&palette)),
            Some(rgb_val(0x37, 0x60, 0x92))
        );
    }

    #[test]
    fn resolve_color_theme_variant_without_tint_returns_base() {
        let color = ColorRef::Theme {
            index: 1,
            tint: None,
        };
        let palette = office_palette();
        assert_eq!(
            resolve_color(&color, Some(&palette)),
            Some(rgb_val(0x00, 0x00, 0x00))
        );
    }

    #[test]
    fn resolve_color_theme_variant_without_a_theme_is_none() {
        let color = ColorRef::Theme {
            index: 4,
            tint: None,
        };
        assert_eq!(resolve_color(&color, None), None);
    }

    #[test]
    fn resolve_color_theme_variant_out_of_range_index_is_none() {
        let color = ColorRef::Theme {
            index: 12,
            tint: None,
        };
        let palette = office_palette();
        assert_eq!(resolve_color(&color, Some(&palette)), None);
    }

    #[test]
    fn resolve_color_indexed_variant_delegates_to_lookup_indexed_color() {
        assert_eq!(
            resolve_color(&ColorRef::Indexed(64), None),
            lookup_indexed_color(64)
        );
        assert_eq!(
            resolve_color(&ColorRef::Indexed(200), None),
            lookup_indexed_color(200)
        );
    }
}
