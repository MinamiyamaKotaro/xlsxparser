//! Parses `xl/styles.xml`'s `<numFmts>`/`<cellXfs>` into a `StyleSheet`
//! (`cellXfs` index -> `ResolvedStyle`), classifying each format as
//! date/time or not.

use crate::error::Error;
use crate::model::{Font, ResolvedStyle, StyleId, StyleSheet};
use crate::parse::{create_secure_reader, optional_attr, read_event, required_attr};
use quick_xml::events::Event;
use std::collections::HashMap;
use std::io::BufRead;
use std::sync::Arc;

/// The built-in numFmtIds (ECMA-376 Part 1 §18.8.30) that represent a
/// date/time. 14-22: built-in date/time formats (e.g. 14 = "mm-dd-yy").
/// 45-47: elapsed time (e.g. 46 = "[h]:mm:ss"). Locale-dependent date
/// formats in the 27-36 range, including Japanese era (wareki) dates, are
/// not handled.
const BUILTIN_DATE_TIME_NUMFMT_IDS: &[u32] = &[14, 15, 16, 17, 18, 19, 20, 21, 22, 45, 46, 47];

/// Parses `xl/styles.xml` and builds a `StyleSheet`. Per ECMA-376 Part 1
/// §18.8.39 `CT_Stylesheet`'s `xsd:sequence`, `<numFmts>` and `<fonts>` both
/// always precede `<cellXfs>` in a schema-valid file, so a single streaming
/// pass suffices: custom format codes and font entries are collected first
/// and are already available by the time `<cellXfs>` is reached.
///
/// Font resolution is deliberately shallow: each `<xf>`'s `fontId` indexes
/// directly into the parsed `<fonts>` list (`applyFont` is not consulted,
/// and `<cellStyleXfs>`/`xfId`-based named-style inheritance is not
/// resolved) — see docs/design/parse/styles.en.md Open Question 3 for why.
pub(crate) fn parse_styles(reader: impl BufRead, path: &str) -> Result<StyleSheet, Error> {
    let mut xml_reader = create_secure_reader(reader);
    let mut buf = Vec::new();
    let mut num_fmts: HashMap<u32, String> = HashMap::new();
    let mut fonts: Vec<Font> = Vec::new();
    let mut stylesheet: StyleSheet = HashMap::new();
    let mut in_fonts = false;
    let mut in_cell_xfs = false;
    let mut next_style_id: StyleId = 0;

    // State for the <font> currently being read (between its start and end
    // tag, mirroring parse/worksheet.rs's per-<c> state pattern).
    let mut cur_font: Option<Font> = None;

    loop {
        let event = read_event(&mut xml_reader, &mut buf, path)?;
        match &event {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"numFmt" => {
                let id_str = required_attr(e, path, "numFmtId")?;
                let format_code = required_attr(e, path, "formatCode")?;
                if let Ok(id) = id_str.parse::<u32>() {
                    num_fmts.insert(id, format_code);
                }
            }
            Event::Start(e) if e.local_name().as_ref() == b"fonts" => {
                in_fonts = true;
            }
            Event::End(e) if e.local_name().as_ref() == b"fonts" => {
                in_fonts = false;
            }
            Event::Start(e) if in_fonts && e.local_name().as_ref() == b"font" => {
                cur_font = Some(Font::default());
            }
            Event::Empty(e) if in_fonts && e.local_name().as_ref() == b"font" => {
                // A <font/> with no child properties at all (unusual, but
                // not schema-invalid): registers Excel's own default.
                fonts.push(Font::default());
            }
            Event::End(e) if in_fonts && e.local_name().as_ref() == b"font" => {
                if let Some(font) = cur_font.take() {
                    fonts.push(font);
                }
            }
            Event::Start(e) | Event::Empty(e)
                if cur_font.is_some() && e.local_name().as_ref() == b"sz" =>
            {
                if let Some(size_pt) =
                    optional_attr(e, path, "val")?.and_then(|v| v.parse::<f64>().ok())
                {
                    cur_font
                        .as_mut()
                        .expect("cur_font.is_some() checked above")
                        .size_pt = size_pt;
                }
            }
            Event::Start(e) | Event::Empty(e)
                if cur_font.is_some() && e.local_name().as_ref() == b"b" =>
            {
                // <b/> (no val) means bold; <b val="0"/>/<b val="false"/>
                // is the rarer explicit "not bold" form (ECMA-376
                // ST_OnOff's boolean-like value space).
                let bold = !matches!(
                    optional_attr(e, path, "val")?.as_deref(),
                    Some("0" | "false")
                );
                cur_font
                    .as_mut()
                    .expect("cur_font.is_some() checked above")
                    .bold = bold;
            }
            Event::Start(e) if e.local_name().as_ref() == b"cellXfs" => {
                in_cell_xfs = true;
            }
            Event::End(e) if e.local_name().as_ref() == b"cellXfs" => {
                in_cell_xfs = false;
            }
            Event::Start(e) | Event::Empty(e)
                if in_cell_xfs && e.local_name().as_ref() == b"xf" =>
            {
                // numFmtId is optional; absent means 0 ("General", not a
                // date). A present-but-unparseable value degrades the same
                // way, per the graceful-degradation policy below.
                let numfmt_id = optional_attr(e, path, "numFmtId")?
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0);
                let is_date_time =
                    is_date_time_format(numfmt_id, num_fmts.get(&numfmt_id).map(String::as_str));
                // fontId absent, unparseable, or out of range (a malformed
                // file referencing a <font> entry that was never defined)
                // all degrade to Font::default() rather than erroring —
                // the same graceful-degradation policy as numFmtId above.
                let font_id = optional_attr(e, path, "fontId")?
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(0);
                let font = fonts.get(font_id).copied().unwrap_or_default();
                stylesheet.insert(
                    next_style_id,
                    Arc::new(ResolvedStyle { is_date_time, font }),
                );
                next_style_id += 1;
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(stylesheet)
}

/// Classifies whether the format identified by `numfmt_id` — and, for a
/// custom format, `format_code` (the lookup result from `num_fmts`, `None`
/// if not found) — represents a date/time.
///
/// - `numfmt_id < 164` (built-in): checked against
///   `BUILTIN_DATE_TIME_NUMFMT_IDS`.
/// - `numfmt_id >= 164` (custom): scans `format_code` heuristically for
///   date/time tokens (`y`, `m`, `d`, `h`, `s`), skipping quoted literal
///   sections (`"..."`) and `\`-escaped literal characters. A bracketed
///   section (`[...]`) is treated as an elapsed-time token (matches, e.g.
///   `[h]`/`[mm]`/`[ss]`) only when its content consists solely of `h`/`m`/
///   `s` letters; any other bracket content (colors like `[Red]`,
///   conditions like `[>100]`, locale tags like `[$-409]`) is discarded
///   whole rather than scanned, so it can never contribute a false-positive
///   date/time letter. This classification is not exhaustive (see
///   docs/design/parse/styles.en.md Open Question 2) but is a simple,
///   linear-time token scan with no backtracking, by design (security
///   review Finding 4: ReDoS mitigation for this untrusted, external-file-
///   controlled input).
/// - If `numfmt_id` is found neither among the built-ins nor in the custom
///   definitions, falls back to `is_date_time: false` rather than erroring.
fn is_date_time_format(numfmt_id: u32, format_code: Option<&str>) -> bool {
    if numfmt_id < 164 {
        return BUILTIN_DATE_TIME_NUMFMT_IDS.contains(&numfmt_id);
    }
    match format_code {
        Some(code) => contains_date_time_token(code),
        None => false,
    }
}

/// Linear single-pass scan for date/time tokens in a custom `formatCode`.
/// See [`is_date_time_format`] for the exclusion rules.
fn contains_date_time_token(format_code: &str) -> bool {
    let mut chars = format_code.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '"' => {
                for c2 in chars.by_ref() {
                    if c2 == '"' {
                        break;
                    }
                }
            }
            '[' => {
                let mut bracket_is_hms = true;
                let mut bracket_nonempty = false;
                for c2 in chars.by_ref() {
                    if c2 == ']' {
                        break;
                    }
                    bracket_nonempty = true;
                    if !matches!(c2.to_ascii_lowercase(), 'h' | 'm' | 's') {
                        bracket_is_hms = false;
                    }
                }
                if bracket_nonempty && bracket_is_hms {
                    return true;
                }
            }
            c if matches!(c.to_ascii_lowercase(), 'y' | 'm' | 'd' | 'h' | 's') => return true,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &[u8]) -> StyleSheet {
        parse_styles(xml, "xl/styles.xml").unwrap()
    }

    #[test]
    fn builtin_date_time_numfmt_is_date() {
        let xml = br#"<styleSheet><cellXfs><xf numFmtId="14"/></cellXfs></styleSheet>"#;
        let sheet = parse(xml);
        assert!(sheet[&0].is_date_time);
    }

    #[test]
    fn builtin_non_date_numfmt_is_not_date() {
        let xml =
            br#"<styleSheet><cellXfs><xf numFmtId="0"/><xf numFmtId="9"/></cellXfs></styleSheet>"#;
        let sheet = parse(xml);
        assert!(!sheet[&0].is_date_time);
        assert!(!sheet[&1].is_date_time);
    }

    #[test]
    fn custom_format_with_date_tokens_is_date() {
        let xml = br#"<styleSheet>
<numFmts><numFmt numFmtId="164" formatCode="yyyy/mm/dd"/></numFmts>
<cellXfs><xf numFmtId="164"/></cellXfs>
</styleSheet>"#;
        let sheet = parse(xml);
        assert!(sheet[&0].is_date_time);
    }

    #[test]
    fn custom_format_without_date_tokens_is_not_date() {
        let xml = br##"<styleSheet>
<numFmts>
<numFmt numFmtId="164" formatCode="#,##0.00"/>
<numFmt numFmtId="165" formatCode="@"/>
</numFmts>
<cellXfs><xf numFmtId="164"/><xf numFmtId="165"/></cellXfs>
</styleSheet>"##;
        let sheet = parse(xml);
        assert!(!sheet[&0].is_date_time);
        assert!(!sheet[&1].is_date_time);
    }

    #[test]
    fn custom_format_with_color_and_condition_brackets_is_not_misclassified() {
        let xml = br#"<styleSheet>
<numFmts><numFmt numFmtId="164" formatCode="[Red]#,##0;[Blue]-#,##0"/></numFmts>
<cellXfs><xf numFmtId="164"/></cellXfs>
</styleSheet>"#;
        let sheet = parse(xml);
        assert!(!sheet[&0].is_date_time);
    }

    #[test]
    fn custom_elapsed_time_bracket_is_date() {
        let xml = br#"<styleSheet>
<numFmts><numFmt numFmtId="164" formatCode="[h]:mm:ss"/></numFmts>
<cellXfs><xf numFmtId="164"/></cellXfs>
</styleSheet>"#;
        let sheet = parse(xml);
        assert!(sheet[&0].is_date_time);
    }

    #[test]
    fn unknown_numfmt_id_falls_back_to_not_date() {
        let xml = br#"<styleSheet><cellXfs><xf numFmtId="999"/></cellXfs></styleSheet>"#;
        let sheet = parse(xml);
        assert!(!sheet[&0].is_date_time);
    }

    #[test]
    fn xf_without_numfmt_id_defaults_to_general() {
        let xml = br#"<styleSheet><cellXfs><xf/></cellXfs></styleSheet>"#;
        let sheet = parse(xml);
        assert!(!sheet[&0].is_date_time);
    }

    #[test]
    fn style_ids_match_cell_xfs_index_order() {
        let xml = br#"<styleSheet><cellXfs><xf numFmtId="0"/><xf numFmtId="14"/><xf numFmtId="9"/></cellXfs></styleSheet>"#;
        let sheet = parse(xml);
        assert_eq!(sheet.len(), 3);
        assert!(!sheet[&0].is_date_time);
        assert!(sheet[&1].is_date_time);
        assert!(!sheet[&2].is_date_time);
    }

    #[test]
    fn non_conformant_num_fmts_after_cell_xfs_does_not_panic() {
        // Schema-invalid order (numFmts after cellXfs); the referenced
        // custom numFmtId isn't known yet when the <xf> is processed, so it
        // degrades to the "not found" case rather than panicking.
        let xml = br#"<styleSheet>
<cellXfs><xf numFmtId="164"/></cellXfs>
<numFmts><numFmt numFmtId="164" formatCode="yyyy/mm/dd"/></numFmts>
</styleSheet>"#;
        let sheet = parse(xml);
        assert!(!sheet[&0].is_date_time);
    }

    #[test]
    fn font_size_and_bold_resolve_via_font_id() {
        let xml = br#"<styleSheet>
<fonts count="2">
<font><sz val="11"/><name val="Calibri"/></font>
<font><b/><sz val="14"/><name val="Calibri"/></font>
</fonts>
<cellXfs><xf fontId="0"/><xf fontId="1"/></cellXfs>
</styleSheet>"#;
        let sheet = parse(xml);
        assert_eq!(
            sheet[&0].font,
            Font {
                size_pt: 11.0,
                bold: false
            }
        );
        assert_eq!(
            sheet[&1].font,
            Font {
                size_pt: 14.0,
                bold: true
            }
        );
    }

    #[test]
    fn bold_val_zero_is_explicitly_not_bold() {
        let xml = br#"<styleSheet>
<fonts count="1"><font><b val="0"/><sz val="12"/></font></fonts>
<cellXfs><xf fontId="0"/></cellXfs>
</styleSheet>"#;
        let sheet = parse(xml);
        assert!(!sheet[&0].font.bold);
    }

    #[test]
    fn xf_without_font_id_defaults_to_first_font() {
        let xml = br#"<styleSheet>
<fonts count="1"><font><b/><sz val="9"/></font></fonts>
<cellXfs><xf/></cellXfs>
</styleSheet>"#;
        let sheet = parse(xml);
        assert_eq!(
            sheet[&0].font,
            Font {
                size_pt: 9.0,
                bold: true
            }
        );
    }

    #[test]
    fn font_id_out_of_range_falls_back_to_default_font() {
        let xml = br#"<styleSheet>
<fonts count="1"><font><sz val="20"/></font></fonts>
<cellXfs><xf fontId="5"/></cellXfs>
</styleSheet>"#;
        let sheet = parse(xml);
        assert_eq!(sheet[&0].font, Font::default());
    }

    #[test]
    fn no_fonts_element_at_all_falls_back_to_default_font() {
        let xml = br#"<styleSheet><cellXfs><xf numFmtId="0"/></cellXfs></styleSheet>"#;
        let sheet = parse(xml);
        assert_eq!(sheet[&0].font, Font::default());
    }

    #[test]
    fn empty_font_element_registers_default_font() {
        let xml = br#"<styleSheet>
<fonts count="1"><font/></fonts>
<cellXfs><xf fontId="0"/></cellXfs>
</styleSheet>"#;
        let sheet = parse(xml);
        assert_eq!(sheet[&0].font, Font::default());
    }
}
