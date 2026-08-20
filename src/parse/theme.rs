// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Parses `xl/theme/theme{N}.xml`'s `<a:clrScheme>` (Issue #76) into a
//! `ThemePalette`. Nothing outside `<clrScheme>` (shape styles, font
//! schemes, ...) is ever interpreted.

use crate::error::Error;
use crate::model::{Rgb, ThemePalette};
use crate::parse::{create_secure_reader, optional_attr, read_event};
use quick_xml::events::{BytesStart, Event};
use std::io::BufRead;

/// The 12 named slots `<clrScheme>` has, listed not in XML declaration
/// order but in the resolved index order `ThemePalette` contracts: slots
/// 0/1 are swapped relative to `<clrScheme>`'s own child order
/// (`dk1,lt1,dk2,lt2,...`) — a well-known trap easy to get wrong, confirmed
/// against real data and Apache POI's `ThemesTable.ThemeElement` enum by a
/// PoC (Issue #76). This table itself doubles as the "name -> output
/// index" mapping.
const SLOT_NAMES: [&str; 12] = [
    "lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6",
    "hlink", "folHlink",
];

/// Parses `xl/theme/theme{N}.xml` and builds a `ThemePalette`. `path` is an
/// already-resolved part path — resolving the actual on-disk path of the
/// `theme{N}.xml` part, and deciding whether to call this function at all
/// (Issue #76's "pay-for-what-you-use": skipped entirely when no
/// `ColorRef::Theme` is referenced), are `pipeline.rs`'s responsibility.
pub(crate) fn parse_theme(reader: impl BufRead, path: &str) -> Result<ThemePalette, Error> {
    let mut xml_reader = create_secure_reader(reader);
    let mut buf = Vec::new();
    let mut slots: [Option<Rgb>; 12] = [None; 12];
    let mut in_clr_scheme = false;
    // Index into SLOT_NAMES/slots for the slot element currently open
    // (between its Start and End tag), mirroring parse/styles.rs's
    // cur_font/cur_xf accumulator pattern.
    let mut current_slot: Option<usize> = None;

    loop {
        let event = read_event(&mut xml_reader, &mut buf, path)?;
        match &event {
            Event::Start(e) if e.local_name().as_ref() == b"clrScheme" => {
                in_clr_scheme = true;
            }
            // Only the 12 colors under <clrScheme> matter to this parser;
            // stop reading the instant its closing tag is seen rather than
            // scanning the rest of theme{N}.xml (font scheme, shape
            // defaults, ...) for nothing.
            Event::End(e) if e.local_name().as_ref() == b"clrScheme" => break,
            // A slot element (e.g. <a:dk1>) always wraps a child color
            // element per ECMA-376's CT_Color, so only its Start form opens
            // a scope here — a malformed self-closing <a:dk1/> would leave
            // this slot unresolved and surface as MissingRequiredElement
            // below, rather than corrupting current_slot's Start/End
            // pairing (Empty never gets a matching End event).
            Event::Start(e) if in_clr_scheme && current_slot.is_none() => {
                current_slot = SLOT_NAMES
                    .iter()
                    .position(|name| e.local_name().as_ref() == name.as_bytes());
            }
            Event::Start(e) | Event::Empty(e)
                if current_slot.is_some()
                    && (e.local_name().as_ref() == b"srgbClr"
                        || e.local_name().as_ref() == b"sysClr") =>
            {
                let idx = current_slot.expect("current_slot.is_some() checked above");
                slots[idx] = Some(resolve_slot_color(SLOT_NAMES[idx], e, path)?);
            }
            Event::End(e) if current_slot.is_some() => {
                let idx = current_slot.expect("current_slot.is_some() checked above");
                if e.local_name().as_ref() == SLOT_NAMES[idx].as_bytes() {
                    current_slot = None;
                }
            }
            Event::Eof => {
                return Err(Error::MissingRequiredElement {
                    path: path.to_string(),
                    name: "clrScheme",
                })
            }
            _ => {}
        }
        buf.clear();
    }

    // ThemePalette is a fixed 12-element array that only carries meaning
    // once fully populated — unlike a missing numFmtId (a legitimately
    // valid state that degrades to None), a <clrScheme> missing any of its
    // 12 required slots is a corrupted file, not ambiguity to tolerate
    // while reading (see docs/design/parse/theme.en.md Error Handling
    // Policy).
    let mut resolved = [Rgb::default(); 12];
    for (i, slot) in slots.into_iter().enumerate() {
        resolved[i] = slot.ok_or_else(|| Error::MissingRequiredElement {
            path: path.to_string(),
            name: "clrScheme",
        })?;
    }
    Ok(ThemePalette(resolved))
}

/// Resolves one slot's color element (`<a:srgbClr>` or `<a:sysClr>`) to a
/// real RGB value. `slot_name` picks the fallback value when the element's
/// value can't be interpreted (below).
///
/// - `<a:srgbClr val="RRGGBB"/>`: parses `val` directly as 6-digit hex.
/// - `<a:sysClr val="..." lastClr="RRGGBB"/>`: `val` (a named system color
///   like `windowText`/`window`) has no OS-independent way to resolve, so
///   it's ignored in favor of `lastClr` (the cached value Excel writes on
///   save — the pragmatic compromise other implementations, including
///   Apache POI, take too).
/// - If the relevant attribute (`val` for `srgbClr`, `lastClr` for
///   `sysClr`) is missing or not valid 6-digit hex: falls back to
///   `#FFFFFF` when `slot_name` is `lt1`/`lt2`, or `#000000` otherwise
///   (`dk1`/`dk2`/`accent*`/`hlink`/`folHlink`) — not an error. This is a
///   defensive path only: real Excel-generated files always populate these
///   attributes (confirmed by scanning every bundled fixture's `<a:sysClr>`
///   elements during design PoC — Issue #76).
fn resolve_slot_color(slot_name: &str, e: &BytesStart<'_>, path: &str) -> Result<Rgb, Error> {
    let attr_name = if e.local_name().as_ref() == b"sysClr" {
        "lastClr"
    } else {
        "val"
    };
    let hex = optional_attr(e, path, attr_name)?.and_then(|v| parse_hex6(&v));
    Ok(hex.unwrap_or_else(|| fallback_for_slot(slot_name)))
}

/// The fallback color for a slot whose value couldn't be interpreted (see
/// [`resolve_slot_color`]). `lt1`/`lt2` are Office's "light" slots
/// (background colors, conventionally white); every other slot —
/// `dk1`/`dk2`/`accent1..6`/`hlink`/`folHlink` — defaults to black.
fn fallback_for_slot(slot_name: &str) -> Rgb {
    match slot_name {
        "lt1" | "lt2" => Rgb {
            r: 0xFF,
            g: 0xFF,
            b: 0xFF,
        },
        _ => Rgb {
            r: 0x00,
            g: 0x00,
            b: 0x00,
        },
    }
}

/// Parses a 6-digit hex RGB string (e.g. `"4F81BD"`). `None` if `s` isn't
/// exactly 6 valid hex digits.
fn parse_hex6(s: &str) -> Option<Rgb> {
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(Rgb {
        r: ((v >> 16) & 0xFF) as u8,
        g: ((v >> 8) & 0xFF) as u8,
        b: (v & 0xFF) as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &[u8]) -> Result<ThemePalette, Error> {
        parse_theme(xml, "xl/theme/theme1.xml")
    }

    const OFFICE_THEME_XML: &[u8] =
        br#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<a:themeElements>
<a:clrScheme name="Office">
<a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>
<a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
<a:dk2><a:srgbClr val="1F497D"/></a:dk2>
<a:lt2><a:srgbClr val="EEECE1"/></a:lt2>
<a:accent1><a:srgbClr val="4F81BD"/></a:accent1>
<a:accent2><a:srgbClr val="C0504D"/></a:accent2>
<a:accent3><a:srgbClr val="9BBB59"/></a:accent3>
<a:accent4><a:srgbClr val="8064A2"/></a:accent4>
<a:accent5><a:srgbClr val="4BACC6"/></a:accent5>
<a:accent6><a:srgbClr val="F79646"/></a:accent6>
<a:hlink><a:srgbClr val="0000FF"/></a:hlink>
<a:folHlink><a:srgbClr val="800080"/></a:folHlink>
</a:clrScheme>
<a:fontScheme name="Office"><a:majorFont><a:latin typeface="Cambria"/></a:majorFont></a:fontScheme>
</a:themeElements>
</a:theme>"#;

    #[test]
    fn real_office_theme_resolves_with_slot_0_1_swapped() {
        // PoC-verified against tests/fixtures/complex/styled_fill_color.xlsx
        // (Issue #76 comment #5352366260): index 0 must be lt1 (white),
        // index 1 must be dk1 (black) — the reverse of <clrScheme>'s own
        // dk1,lt1 declaration order.
        let palette = parse(OFFICE_THEME_XML).unwrap();
        assert_eq!(
            palette.0[0],
            Rgb {
                r: 0xFF,
                g: 0xFF,
                b: 0xFF
            },
            "index 0 must be lt1"
        );
        assert_eq!(
            palette.0[1],
            Rgb {
                r: 0x00,
                g: 0x00,
                b: 0x00
            },
            "index 1 must be dk1"
        );
        assert_eq!(
            palette.0[4],
            Rgb {
                r: 0x4F,
                g: 0x81,
                b: 0xBD
            },
            "index 4 must be accent1"
        );
        assert_eq!(
            palette.0[11],
            Rgb {
                r: 0x80,
                g: 0x00,
                b: 0x80
            },
            "index 11 must be folHlink"
        );
    }

    #[test]
    fn srgb_clr_resolves_directly() {
        let xml = br#"<a:clrScheme><a:dk1><a:sysClr lastClr="000000"/></a:dk1><a:lt1><a:sysClr lastClr="FFFFFF"/></a:lt1><a:dk2><a:sysClr lastClr="000000"/></a:dk2><a:lt2><a:sysClr lastClr="FFFFFF"/></a:lt2><a:accent1><a:srgbClr val="4F81BD"/></a:accent1><a:accent2><a:sysClr lastClr="000000"/></a:accent2><a:accent3><a:sysClr lastClr="000000"/></a:accent3><a:accent4><a:sysClr lastClr="000000"/></a:accent4><a:accent5><a:sysClr lastClr="000000"/></a:accent5><a:accent6><a:sysClr lastClr="000000"/></a:accent6><a:hlink><a:sysClr lastClr="000000"/></a:hlink><a:folHlink><a:sysClr lastClr="000000"/></a:folHlink></a:clrScheme>"#;
        let palette = parse(xml).unwrap();
        assert_eq!(
            palette.0[4],
            Rgb {
                r: 0x4F,
                g: 0x81,
                b: 0xBD
            }
        );
    }

    #[test]
    fn sys_clr_uses_last_clr_not_val() {
        let xml = br#"<a:clrScheme><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:sysClr lastClr="000000"/></a:dk2><a:lt2><a:sysClr lastClr="FFFFFF"/></a:lt2><a:accent1><a:sysClr lastClr="000000"/></a:accent1><a:accent2><a:sysClr lastClr="000000"/></a:accent2><a:accent3><a:sysClr lastClr="000000"/></a:accent3><a:accent4><a:sysClr lastClr="000000"/></a:accent4><a:accent5><a:sysClr lastClr="000000"/></a:accent5><a:accent6><a:sysClr lastClr="000000"/></a:accent6><a:hlink><a:sysClr lastClr="000000"/></a:hlink><a:folHlink><a:sysClr lastClr="000000"/></a:folHlink></a:clrScheme>"#;
        let palette = parse(xml).unwrap();
        // lastClr's value (#000000), not val's (windowText, unresolvable).
        assert_eq!(
            palette.0[1],
            Rgb {
                r: 0x00,
                g: 0x00,
                b: 0x00
            }
        );
        assert_eq!(
            palette.0[0],
            Rgb {
                r: 0xFF,
                g: 0xFF,
                b: 0xFF
            }
        );
    }

    fn xml_with_dk1_lt1_replaced(dk1: &str, lt1: &str) -> String {
        format!(
            r#"<a:clrScheme><a:dk1>{dk1}</a:dk1><a:lt1>{lt1}</a:lt1><a:dk2><a:sysClr lastClr="000000"/></a:dk2><a:lt2><a:sysClr lastClr="FFFFFF"/></a:lt2><a:accent1><a:sysClr lastClr="000000"/></a:accent1><a:accent2><a:sysClr lastClr="000000"/></a:accent2><a:accent3><a:sysClr lastClr="000000"/></a:accent3><a:accent4><a:sysClr lastClr="000000"/></a:accent4><a:accent5><a:sysClr lastClr="000000"/></a:accent5><a:accent6><a:sysClr lastClr="000000"/></a:accent6><a:hlink><a:sysClr lastClr="000000"/></a:hlink><a:folHlink><a:sysClr lastClr="000000"/></a:folHlink></a:clrScheme>"#
        )
    }

    #[test]
    fn sys_clr_without_last_clr_falls_back_by_slot() {
        // A path that never fires on real fixtures (every bundled
        // <a:sysClr> carries lastClr — Issue #76 PoC), so exercised here
        // against synthetic XML.
        let xml = xml_with_dk1_lt1_replaced(
            r#"<a:sysClr val="windowText"/>"#,
            r#"<a:sysClr val="window"/>"#,
        );
        let palette = parse(xml.as_bytes()).unwrap();
        assert_eq!(
            palette.0[1], // dk1
            Rgb {
                r: 0x00,
                g: 0x00,
                b: 0x00
            }
        );
        assert_eq!(
            palette.0[0], // lt1
            Rgb {
                r: 0xFF,
                g: 0xFF,
                b: 0xFF
            }
        );
    }

    #[test]
    fn invalid_hex_last_clr_falls_back_by_slot_without_panicking() {
        let xml = xml_with_dk1_lt1_replaced(
            r#"<a:sysClr val="windowText" lastClr="ZZZZZZ"/>"#,
            r#"<a:sysClr val="window" lastClr="ZZZZZZ"/>"#,
        );
        let palette = parse(xml.as_bytes()).unwrap();
        assert_eq!(
            palette.0[1],
            Rgb {
                r: 0x00,
                g: 0x00,
                b: 0x00
            }
        );
        assert_eq!(
            palette.0[0],
            Rgb {
                r: 0xFF,
                g: 0xFF,
                b: 0xFF
            }
        );
    }

    #[test]
    fn wrong_length_and_non_ascii_last_clr_also_fall_back_by_slot() {
        // Distinct from invalid_hex_last_clr_falls_back_by_slot_without_panicking
        // above (6 valid ASCII chars that just aren't hex digits, e.g.
        // "ZZZZZZ" — fails at the from_str_radix step): this exercises
        // parse_hex6's own length/ASCII guard, which a too-short value and
        // a non-ASCII value both fail before from_str_radix is ever
        // reached.
        let xml = xml_with_dk1_lt1_replaced(
            r#"<a:sysClr val="windowText" lastClr="12"/>"#,
            r#"<a:sysClr val="window" lastClr="日本語カラー"/>"#,
        );
        let palette = parse(xml.as_bytes()).unwrap();
        assert_eq!(
            palette.0[1],
            Rgb {
                r: 0x00,
                g: 0x00,
                b: 0x00
            }
        );
        assert_eq!(
            palette.0[0],
            Rgb {
                r: 0xFF,
                g: 0xFF,
                b: 0xFF
            }
        );
    }

    #[test]
    fn missing_slot_is_missing_required_element() {
        // <a:accent3> omitted entirely — a structurally broken theme part.
        let xml = br#"<a:clrScheme><a:dk1><a:sysClr lastClr="000000"/></a:dk1><a:lt1><a:sysClr lastClr="FFFFFF"/></a:lt1><a:dk2><a:sysClr lastClr="000000"/></a:dk2><a:lt2><a:sysClr lastClr="FFFFFF"/></a:lt2><a:accent1><a:sysClr lastClr="000000"/></a:accent1><a:accent2><a:sysClr lastClr="000000"/></a:accent2><a:accent4><a:sysClr lastClr="000000"/></a:accent4><a:accent5><a:sysClr lastClr="000000"/></a:accent5><a:accent6><a:sysClr lastClr="000000"/></a:accent6><a:hlink><a:sysClr lastClr="000000"/></a:hlink><a:folHlink><a:sysClr lastClr="000000"/></a:folHlink></a:clrScheme>"#;
        let err = parse(xml).unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement {
                name: "clrScheme",
                ..
            }
        ));
    }

    #[test]
    fn missing_clr_scheme_entirely_is_missing_required_element() {
        let xml = br#"<a:theme><a:themeElements><a:fontScheme/></a:themeElements></a:theme>"#;
        let err = parse(xml).unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement {
                name: "clrScheme",
                ..
            }
        ));
    }

    #[test]
    fn namespace_prefix_other_than_a_still_resolves_by_local_name() {
        let xml = String::from_utf8(OFFICE_THEME_XML.to_vec())
            .unwrap()
            .replace("a:", "drawing:");
        let palette = parse(xml.as_bytes()).unwrap();
        assert_eq!(
            palette.0[4],
            Rgb {
                r: 0x4F,
                g: 0x81,
                b: 0xBD
            }
        );
    }

    #[test]
    fn stops_reading_after_clr_scheme_closes_rather_than_scanning_the_rest() {
        // If parse_theme kept reading past </a:clrScheme>, the malformed
        // <unclosed> tag afterward would surface as an XmlParse error
        // instead of a clean Ok result.
        let xml = xml_with_dk1_lt1_replaced(
            r#"<a:sysClr lastClr="000000"/>"#,
            r#"<a:sysClr lastClr="FFFFFF"/>"#,
        )
        .replace("</a:clrScheme>", "</a:clrScheme><a:fontScheme><unclosed>");
        assert!(parse(xml.as_bytes()).is_ok());
    }

    #[test]
    fn non_self_closing_color_element_resolves_the_same_as_self_closing() {
        // ECMA-376 permits <a:srgbClr val="..">..</a:srgbClr>` as well as
        // the self-closing form every other test uses — both must resolve
        // identically, since resolve_slot_color only reads the start tag's
        // attributes either way.
        let xml = xml_with_dk1_lt1_replaced(
            r#"<a:sysClr lastClr="000000"></a:sysClr>"#,
            r#"<a:sysClr lastClr="FFFFFF"></a:sysClr>"#,
        );
        let palette = parse(xml.as_bytes()).unwrap();
        assert_eq!(
            palette.0[1],
            Rgb {
                r: 0x00,
                g: 0x00,
                b: 0x00
            }
        );
        assert_eq!(
            palette.0[0],
            Rgb {
                r: 0xFF,
                g: 0xFF,
                b: 0xFF
            }
        );
    }

    #[test]
    fn xml_syntax_error_inside_clr_scheme_propagates_as_xml_parse() {
        // Distinct from missing_clr_scheme_entirely/missing_slot (both
        // well-formed XML that's merely incomplete): here the XML itself
        // is malformed, exercising read_event's own Err propagation via
        // the `?` at the top of parse_theme's loop.
        let xml = br#"<a:clrScheme><a:dk1><unclosed></a:dk1></a:clrScheme>"#;
        let err = parse(xml).unwrap_err();
        assert!(matches!(err, Error::XmlParse { .. }));
    }
}
