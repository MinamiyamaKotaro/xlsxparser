//! Parses `xl/styles.xml`'s `<numFmts>`/`<fonts>`/`<cellXfs>` into a
//! `StyleSheet` (`cellXfs` index -> `ResolvedStyle`), classifying each
//! format as date/time or not and resolving each `<xf>`'s font/wrap-text.

use crate::error::Error;
use crate::model::{Alignment, Font, ResolvedStyle, StyleId, StyleSheet};
use crate::parse::{create_secure_reader, optional_attr, read_event, required_attr};
use quick_xml::events::{BytesStart, Event};
use std::collections::HashMap;
use std::io::BufRead;
use std::sync::Arc;

/// The built-in numFmtIds (ECMA-376 Part 1 §18.8.30) that represent a
/// date/time. 14-22: built-in date/time formats (e.g. 14 = "mm-dd-yy").
/// 45-47: elapsed time (e.g. 46 = "[h]:mm:ss"). Locale-dependent date
/// formats in the 27-36 range, including Japanese era (wareki) dates, are
/// not handled.
const BUILTIN_DATE_TIME_NUMFMT_IDS: &[u32] = &[14, 15, 16, 17, 18, 19, 20, 21, 22, 45, 46, 47];

/// The built-in format-code strings ECMA-376 Part 1 §18.8.30 defines for
/// `numFmtId` values 1-49 (Issue #41). `0` ("General") is deliberately
/// omitted — it resolves to `ResolvedStyle::number_format: None`, the same
/// "nothing special to report" treatment as an unrecognized ID, rather than
/// `Some("General")` (see [`resolve_number_format`]). IDs 23-36 and 50-163
/// are reserved for application/international use with no format code
/// defined by the spec itself, so they are absent from this table and also
/// fall back to `None`.
const BUILTIN_NUMFMT_CODES: &[(u32, &str)] = &[
    (1, "0"),
    (2, "0.00"),
    (3, "#,##0"),
    (4, "#,##0.00"),
    (5, "$#,##0_);($#,##0)"),
    (6, "$#,##0_);[Red]($#,##0)"),
    (7, "$#,##0.00_);($#,##0.00)"),
    (8, "$#,##0.00_);[Red]($#,##0.00)"),
    (9, "0%"),
    (10, "0.00%"),
    (11, "0.00E+00"),
    (12, "# ?/?"),
    (13, "# ??/??"),
    (14, "mm-dd-yy"),
    (15, "d-mmm-yy"),
    (16, "d-mmm"),
    (17, "mmm-yy"),
    (18, "h:mm AM/PM"),
    (19, "h:mm:ss AM/PM"),
    (20, "h:mm"),
    (21, "h:mm:ss"),
    (22, "m/d/yy h:mm"),
    (37, "#,##0_);(#,##0)"),
    (38, "#,##0_);[Red](#,##0)"),
    (39, "#,##0.00_);(#,##0.00)"),
    (40, "#,##0.00_);[Red](#,##0.00)"),
    (41, "_(* #,##0_);_(* (#,##0);_(* \"-\"_);_(@_)"),
    (42, "_($* #,##0_);_($* (#,##0);_($* \"-\"_);_(@_)"),
    (43, "_(* #,##0.00_);_(* (#,##0.00);_(* \"-\"??_);_(@_)"),
    (44, "_($* #,##0.00_);_($* (#,##0.00);_($* \"-\"??_);_(@_)"),
    (45, "mm:ss"),
    (46, "[h]:mm:ss"),
    (47, "mm:ss.0"),
    (48, "##0.0E+0"),
    (49, "@"),
];

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
    // Caches the resolved Arc<str> per numFmtId (Issue #41), so a format
    // code shared by many <xf> entries is allocated at most once per unique
    // numFmtId rather than once per StyleId.
    let mut resolved_formats: HashMap<u32, Arc<str>> = HashMap::new();
    let mut in_fonts = false;
    let mut in_cell_xfs = false;
    let mut next_style_id: StyleId = 0;

    // State for the <font> currently being read (between its start and end
    // tag, mirroring parse/worksheet.rs's per-<c> state pattern).
    let mut cur_font: Option<Font> = None;
    // Same pattern for the <xf> currently being read.
    let mut cur_xf: Option<CurXf> = None;

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
                // is the rarer explicit "not bold" form. `val`'s type is
                // plain `xsd:boolean` (ECMA-376 Part 1 CT_BooleanProperty),
                // not ST_OnOff — "off"/"on" are not valid lexical forms
                // here (PR #49 review discussion), so only "0"/"false" are
                // treated as false.
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
            // <xf> may be self-closing (no <alignment>/<protection>
            // children) or a Start...End pair wrapping them. Either way,
            // numFmtId/fontId live on the <xf> start tag itself, so they're
            // read immediately; wrap_text (from a child <alignment>) is
            // only known once that child — if any — has been seen, so the
            // ResolvedStyle itself is only built at End (or immediately for
            // the self-closing form, where no child could follow).
            Event::Start(e) if in_cell_xfs && e.local_name().as_ref() == b"xf" => {
                let (numfmt_id, font_id) = read_xf_ids(e, path)?;
                cur_xf = Some(CurXf {
                    numfmt_id,
                    font_id,
                    wrap_text: false,
                    horizontal_alignment: Alignment::General,
                });
            }
            Event::Empty(e) if in_cell_xfs && e.local_name().as_ref() == b"xf" => {
                let (numfmt_id, font_id) = read_xf_ids(e, path)?;
                push_resolved_style(
                    &mut stylesheet,
                    &mut next_style_id,
                    &num_fmts,
                    &fonts,
                    &mut resolved_formats,
                    numfmt_id,
                    font_id,
                    false,
                    Alignment::General,
                );
            }
            Event::End(e) if in_cell_xfs && e.local_name().as_ref() == b"xf" => {
                if let Some(xf) = cur_xf.take() {
                    push_resolved_style(
                        &mut stylesheet,
                        &mut next_style_id,
                        &num_fmts,
                        &fonts,
                        &mut resolved_formats,
                        xf.numfmt_id,
                        xf.font_id,
                        xf.wrap_text,
                        xf.horizontal_alignment,
                    );
                }
            }
            Event::Start(e) | Event::Empty(e)
                if cur_xf.is_some() && e.local_name().as_ref() == b"alignment" =>
            {
                // wrapText is a plain attribute (not a <val>-wrapped
                // CT_BooleanProperty like <b>), absent -> not wrapped;
                // only "1"/"true" count as wrapped, matching xsd:boolean's
                // true forms.
                let wrap_text = matches!(
                    optional_attr(e, path, "wrapText")?.as_deref(),
                    Some("1" | "true")
                );
                // Read off the same start tag as wrapText (Issue #42) — no
                // additional <xf> scan. `horizontal`'s value space is
                // ECMA-376 ST_HorizontalAlignmentValues; "general", an
                // absent attribute, or any unrecognized value all fall back
                // to Alignment::General.
                let horizontal_alignment = match optional_attr(e, path, "horizontal")?.as_deref() {
                    Some("left") => Alignment::Left,
                    Some("center") => Alignment::Center,
                    Some("right") => Alignment::Right,
                    Some("fill") => Alignment::Fill,
                    Some("justify") => Alignment::Justify,
                    Some("centerContinuous") => Alignment::CenterContinuous,
                    Some("distributed") => Alignment::Distributed,
                    _ => Alignment::General,
                };
                let xf = cur_xf.as_mut().expect("cur_xf.is_some() checked above");
                xf.wrap_text = wrap_text;
                xf.horizontal_alignment = horizontal_alignment;
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(stylesheet)
}

/// The `<xf>` currently being read (between its start and end tag, when it
/// isn't self-closing) — mirrors `cur_font`'s pattern.
struct CurXf {
    numfmt_id: u32,
    font_id: usize,
    wrap_text: bool,
    horizontal_alignment: Alignment,
}

/// Reads `numFmtId`/`fontId` off an `<xf>` start tag. Both are optional;
/// absent, unparseable, or (for `fontId`) out of range against the parsed
/// `<fonts>` list all degrade gracefully rather than erroring — resolved
/// by `push_resolved_style`.
fn read_xf_ids(e: &BytesStart<'_>, path: &str) -> Result<(u32, usize), Error> {
    let numfmt_id = optional_attr(e, path, "numFmtId")?
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let font_id = optional_attr(e, path, "fontId")?
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    Ok((numfmt_id, font_id))
}

/// Resolves one `<xf>`'s `ResolvedStyle` and inserts it at `next_style_id`
/// (incrementing it), the same "resolve once, on the way out" step
/// whether the `<xf>` was self-closing or had children.
#[allow(clippy::too_many_arguments)]
fn push_resolved_style(
    stylesheet: &mut StyleSheet,
    next_style_id: &mut StyleId,
    num_fmts: &HashMap<u32, String>,
    fonts: &[Font],
    resolved_formats: &mut HashMap<u32, Arc<str>>,
    numfmt_id: u32,
    font_id: usize,
    wrap_text: bool,
    horizontal_alignment: Alignment,
) {
    let is_date_time = is_date_time_format(numfmt_id, num_fmts.get(&numfmt_id).map(String::as_str));
    let font = fonts.get(font_id).copied().unwrap_or_default();
    let number_format = resolve_number_format(numfmt_id, num_fmts, resolved_formats);
    stylesheet.insert(
        *next_style_id,
        Arc::new(ResolvedStyle {
            is_date_time,
            font,
            wrap_text,
            horizontal_alignment,
            number_format,
        }),
    );
    *next_style_id += 1;
}

/// Resolves `numfmt_id` to its format-code string (Issue #41), same ID-range
/// partition `is_date_time_format` already uses: `numfmt_id < 164` looks up
/// [`BUILTIN_NUMFMT_CODES`], `numfmt_id >= 164` looks up `num_fmts` (built
/// from `<numFmts>`) — per ECMA-376, a custom format is never assigned an ID
/// below 164, so there is no precedence question between the two sources.
/// `None` for `numFmtId=0` ("General") or any ID found in neither table
/// (graceful degradation, matching `is_date_time_format`'s policy for an
/// unresolvable ID).
///
/// `resolved_formats` caches the `Arc<str>` per `numfmt_id`, so a format
/// code shared by many `<xf>` entries (and thus many `StyleId`s) is only
/// ever allocated once per unique `numfmt_id` in the file, not once per
/// `StyleId` — the performance requirement Issue #41 states explicitly.
fn resolve_number_format(
    numfmt_id: u32,
    num_fmts: &HashMap<u32, String>,
    resolved_formats: &mut HashMap<u32, Arc<str>>,
) -> Option<Arc<str>> {
    if let Some(cached) = resolved_formats.get(&numfmt_id) {
        return Some(cached.clone());
    }
    let code = if numfmt_id < 164 {
        BUILTIN_NUMFMT_CODES
            .iter()
            .find(|(id, _)| *id == numfmt_id)
            .map(|(_, code)| *code)
    } else {
        num_fmts.get(&numfmt_id).map(String::as_str)
    }?;
    let arc: Arc<str> = Arc::from(code);
    resolved_formats.insert(numfmt_id, arc.clone());
    Some(arc)
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

    #[test]
    fn xf_with_wrap_text_alignment_resolves_wrap_text_true() {
        let xml = br#"<styleSheet>
<cellXfs><xf numFmtId="0"><alignment wrapText="1"/></xf></cellXfs>
</styleSheet>"#;
        let sheet = parse(xml);
        assert!(sheet[&0].wrap_text);
    }

    #[test]
    fn xf_with_alignment_but_no_wrap_text_attr_is_false() {
        let xml = br#"<styleSheet>
<cellXfs><xf numFmtId="0"><alignment horizontal="center"/></xf></cellXfs>
</styleSheet>"#;
        let sheet = parse(xml);
        assert!(!sheet[&0].wrap_text);
    }

    #[test]
    fn xf_with_wrap_text_zero_is_false() {
        let xml = br#"<styleSheet>
<cellXfs><xf numFmtId="0"><alignment wrapText="0"/></xf></cellXfs>
</styleSheet>"#;
        let sheet = parse(xml);
        assert!(!sheet[&0].wrap_text);
    }

    #[test]
    fn self_closing_xf_with_no_alignment_child_is_wrap_text_false() {
        let xml = br#"<styleSheet><cellXfs><xf numFmtId="0"/></cellXfs></styleSheet>"#;
        let sheet = parse(xml);
        assert!(!sheet[&0].wrap_text);
    }

    #[test]
    fn xf_with_alignment_still_resolves_font_and_num_fmt_correctly() {
        // Proves the Start/End restructuring for <xf> (needed to support a
        // child <alignment>) didn't regress font/numFmt resolution, which
        // both read attributes off the <xf> start tag itself.
        let xml = br#"<styleSheet>
<numFmts><numFmt numFmtId="164" formatCode="yyyy/mm/dd"/></numFmts>
<fonts count="2">
<font><sz val="11"/></font>
<font><b/><sz val="14"/></font>
</fonts>
<cellXfs><xf numFmtId="164" fontId="1"><alignment wrapText="1"/></xf></cellXfs>
</styleSheet>"#;
        let sheet = parse(xml);
        assert!(sheet[&0].is_date_time);
        assert_eq!(
            sheet[&0].font,
            Font {
                size_pt: 14.0,
                bold: true
            }
        );
        assert!(sheet[&0].wrap_text);
    }

    #[test]
    fn horizontal_alignment_resolves_each_known_value() {
        let xml = br#"<styleSheet><cellXfs>
<xf numFmtId="0"><alignment horizontal="left"/></xf>
<xf numFmtId="0"><alignment horizontal="center"/></xf>
<xf numFmtId="0"><alignment horizontal="right"/></xf>
<xf numFmtId="0"><alignment horizontal="fill"/></xf>
<xf numFmtId="0"><alignment horizontal="justify"/></xf>
<xf numFmtId="0"><alignment horizontal="centerContinuous"/></xf>
<xf numFmtId="0"><alignment horizontal="distributed"/></xf>
</cellXfs></styleSheet>"#;
        let sheet = parse(xml);
        assert_eq!(sheet[&0].horizontal_alignment, Alignment::Left);
        assert_eq!(sheet[&1].horizontal_alignment, Alignment::Center);
        assert_eq!(sheet[&2].horizontal_alignment, Alignment::Right);
        assert_eq!(sheet[&3].horizontal_alignment, Alignment::Fill);
        assert_eq!(sheet[&4].horizontal_alignment, Alignment::Justify);
        assert_eq!(sheet[&5].horizontal_alignment, Alignment::CenterContinuous);
        assert_eq!(sheet[&6].horizontal_alignment, Alignment::Distributed);
    }

    #[test]
    fn horizontal_alignment_general_absent_and_unknown_all_resolve_to_general() {
        let xml = br#"<styleSheet><cellXfs>
<xf numFmtId="0"><alignment horizontal="general"/></xf>
<xf numFmtId="0"><alignment vertical="center"/></xf>
<xf numFmtId="0"><alignment horizontal="notARealValue"/></xf>
</cellXfs></styleSheet>"#;
        let sheet = parse(xml);
        assert_eq!(sheet[&0].horizontal_alignment, Alignment::General);
        assert_eq!(sheet[&1].horizontal_alignment, Alignment::General);
        assert_eq!(sheet[&2].horizontal_alignment, Alignment::General);
    }

    #[test]
    fn self_closing_xf_with_no_alignment_child_is_horizontal_alignment_general() {
        let xml = br#"<styleSheet><cellXfs><xf numFmtId="0"/></cellXfs></styleSheet>"#;
        let sheet = parse(xml);
        assert_eq!(sheet[&0].horizontal_alignment, Alignment::General);
    }

    #[test]
    fn wrap_text_and_horizontal_alignment_both_resolve_from_the_same_alignment_element() {
        let xml = br#"<styleSheet>
<cellXfs><xf numFmtId="0"><alignment wrapText="1" horizontal="center"/></xf></cellXfs>
</styleSheet>"#;
        let sheet = parse(xml);
        assert!(sheet[&0].wrap_text);
        assert_eq!(sheet[&0].horizontal_alignment, Alignment::Center);
    }

    #[test]
    fn builtin_numfmt_resolves_expected_code() {
        let xml = br#"<styleSheet><cellXfs><xf numFmtId="9"/></cellXfs></styleSheet>"#;
        let sheet = parse(xml);
        assert_eq!(sheet[&0].number_format.as_deref(), Some("0%"));
    }

    #[test]
    fn custom_numfmt_resolves_from_num_fmts() {
        let xml = br##"<styleSheet>
<numFmts><numFmt numFmtId="164" formatCode="#,##0.00"/></numFmts>
<cellXfs><xf numFmtId="164"/></cellXfs>
</styleSheet>"##;
        let sheet = parse(xml);
        assert_eq!(sheet[&0].number_format.as_deref(), Some("#,##0.00"));
    }

    #[test]
    fn general_and_missing_and_unknown_numfmt_id_resolve_to_none() {
        let xml = br#"<styleSheet><cellXfs>
<xf numFmtId="0"/>
<xf/>
<xf numFmtId="30"/>
<xf numFmtId="999"/>
</cellXfs></styleSheet>"#;
        let sheet = parse(xml);
        assert_eq!(sheet[&0].number_format, None);
        assert_eq!(sheet[&1].number_format, None);
        assert_eq!(sheet[&2].number_format, None);
        assert_eq!(sheet[&3].number_format, None);
    }

    #[test]
    fn date_time_style_still_carries_its_number_format() {
        let xml = br#"<styleSheet><cellXfs><xf numFmtId="14"/></cellXfs></styleSheet>"#;
        let sheet = parse(xml);
        assert!(sheet[&0].is_date_time);
        assert_eq!(sheet[&0].number_format.as_deref(), Some("mm-dd-yy"));
    }

    #[test]
    fn same_numfmt_id_across_styles_shares_the_arc() {
        let xml = br#"<styleSheet>
<fonts count="2"><font><sz val="11"/></font><font><b/><sz val="14"/></font></fonts>
<cellXfs><xf numFmtId="9" fontId="0"/><xf numFmtId="9" fontId="1"/></cellXfs>
</styleSheet>"#;
        let sheet = parse(xml);
        let a = sheet[&0].number_format.as_ref().unwrap();
        let b = sheet[&1].number_format.as_ref().unwrap();
        assert!(Arc::ptr_eq(a, b));
    }

    #[test]
    fn multiple_xf_forms_mixed_self_closing_and_with_children() {
        // A file mixing self-closing <xf/> entries with <xf>...</xf>
        // entries that carry an <alignment> child must still assign
        // StyleIds in cellXfs order.
        let xml = br#"<styleSheet>
<cellXfs>
<xf numFmtId="0"/>
<xf numFmtId="0"><alignment wrapText="1"/></xf>
<xf numFmtId="0"/>
</cellXfs>
</styleSheet>"#;
        let sheet = parse(xml);
        assert_eq!(sheet.len(), 3);
        assert!(!sheet[&0].wrap_text);
        assert!(sheet[&1].wrap_text);
        assert!(!sheet[&2].wrap_text);
    }
}
