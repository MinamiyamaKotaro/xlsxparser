//! Parses `xl/styles.xml`'s `<numFmts>`/`<cellXfs>` into a `StyleSheet`
//! (`cellXfs` index -> `ResolvedStyle`), classifying each format as
//! date/time or not.

use crate::error::Error;
use crate::model::{ResolvedStyle, StyleId, StyleSheet};
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
/// §18.8.39 `CT_Stylesheet`'s `xsd:sequence`, `<numFmts>` always precedes
/// `<cellXfs>` in a schema-valid file, so a single streaming pass suffices:
/// custom format codes are collected first and are already available by the
/// time `<cellXfs>` is reached.
#[allow(dead_code)]
pub(crate) fn parse_styles(reader: impl BufRead, path: &str) -> Result<StyleSheet, Error> {
    let mut xml_reader = create_secure_reader(reader);
    let mut buf = Vec::new();
    let mut num_fmts: HashMap<u32, String> = HashMap::new();
    let mut stylesheet: StyleSheet = HashMap::new();
    let mut in_cell_xfs = false;
    let mut next_style_id: StyleId = 0;

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
                stylesheet.insert(next_style_id, Arc::new(ResolvedStyle { is_date_time }));
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
}
