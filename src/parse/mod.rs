// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! XML parsing layer: consolidates the `quick-xml` dependency. Secure
//! `Reader` construction, XML error conversion, and helpers shared by every
//! submodule live here; each submodule interprets one OOXML part's
//! structure.

mod drawing;
mod relationships;
mod shared_strings;
mod styles;
mod theme;
mod workbook;
mod worksheet;

pub(crate) use drawing::parse_drawing;
pub(crate) use relationships::{parse_relationships, TargetMode};
pub(crate) use shared_strings::{parse_shared_strings, SharedStringTable};
pub(crate) use styles::parse_styles;
pub(crate) use theme::parse_theme;
pub(crate) use workbook::parse_workbook_xml;
pub(crate) use worksheet::{parse_worksheet, PendingHyperlink, PendingSharedString, PendingStyle};

use crate::error::Error;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::io::BufRead;

/// The sole gateway for constructing a `Reader` with XXE mitigations
/// applied.
///
/// `quick-xml` is a non-validating parser that, even in its default
/// configuration, never fetches external entities or an external DTD
/// subset — so classic XXE (local file disclosure, SSRF) cannot occur in the
/// first place. Even so, the requirement to "disable external entity
/// expansion during XML parsing" should not remain an implicit assumption;
/// this function exists to make that setting explicit, and [`read_event`]
/// backs it with an active, verifiable check on every event read.
///
/// Sets `trim_text(false)` so element text is never auto-trimmed — needed to
/// not lose shared strings' `xml:space="preserve"`; whether to actually
/// preserve whitespace is decided per submodule.
pub(crate) fn create_secure_reader<R: BufRead>(inner: R) -> Reader<R> {
    let mut reader = Reader::from_reader(inner);
    reader.config_mut().trim_text(false);
    reader
}

/// The sole gateway for converting `quick_xml::Error` into
/// `crate::error::Error`. A limit-exceeded error from `container::sanitize`'s
/// `BoundedReader` (Zip Bomb protection) propagates up wrapped as an
/// `io::Error` inside `quick_xml::Error::Io`, so this first downcasts to
/// `container::sanitize::LimitExceeded` and, if it matches, returns
/// `Error::ZipBombDetected`. Otherwise it wraps the error as
/// `Error::XmlParse`, type-erased per `error.rs`'s policy of never exposing
/// `quick_xml::Error` directly in the public API.
pub(crate) fn convert_xml_error(path: &str, err: quick_xml::Error) -> Error {
    if let quick_xml::Error::Io(io_err) = &err {
        if let Some(limit) = io_err
            .get_ref()
            .and_then(|e| e.downcast_ref::<crate::container::sanitize::LimitExceeded>())
        {
            return Error::ZipBombDetected {
                limit: limit.limit,
                actual: limit.actual,
            };
        }
    }
    Error::XmlParse {
        path: path.to_string(),
        source: Box::new(err),
    }
}

/// The sole gateway for reading events. Calls `reader.read_event_into(buf)`,
/// converts any error via [`convert_xml_error`], and — if the returned
/// `Event` is `Event::DocType` (a `<!DOCTYPE ...>` declaration) — returns
/// `Error::DoctypeRejected` unconditionally without interpreting its content
/// at all (fail closed).
///
/// None of OOXML's `_rels`/`workbook.xml`/`sharedStrings.xml`/`styles.xml`/
/// `sheetX.xml` parts ever carry a DOCTYPE declaration per spec, so this
/// check never rejects a legitimate `.xlsx`. It acts as an independent layer
/// of defense that keeps working even if quick-xml's non-validating
/// behavior were ever broken by a future version change or a switch to a
/// different parser, by cutting processing off the moment a DOCTYPE
/// declaration's mere presence is detected at the XML syntax level. Every
/// module under `parse/` reads events only through this function, never
/// calling `reader.read_event_into` directly.
pub(crate) fn read_event<'a>(
    reader: &mut Reader<impl BufRead>,
    buf: &'a mut Vec<u8>,
    path: &str,
) -> Result<Event<'a>, Error> {
    let event = reader
        .read_event_into(buf)
        .map_err(|err| convert_xml_error(path, err))?;
    if matches!(event, Event::DocType(_)) {
        return Err(Error::DoctypeRejected {
            path: path.to_string(),
        });
    }
    Ok(event)
}

/// Reads attribute `name` from `start`. Returns
/// `Error::MissingRequiredElement` if it is absent, or wraps a malformed
/// attribute (invalid UTF-8, a stray `&` not part of a valid entity, etc.)
/// as `Error::XmlParse`.
pub(crate) fn required_attr(
    start: &BytesStart<'_>,
    path: &str,
    name: &'static str,
) -> Result<String, Error> {
    for attr in start.attributes() {
        let attr = attr.map_err(|err| Error::XmlParse {
            path: path.to_string(),
            source: Box::new(err),
        })?;
        if attr.key.as_ref() == name.as_bytes() {
            let value = attr
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map_err(|err| Error::XmlParse {
                    path: path.to_string(),
                    source: Box::new(err),
                })?;
            return Ok(value.into_owned());
        }
    }
    Err(Error::MissingRequiredElement {
        path: path.to_string(),
        name,
    })
}

/// Like [`required_attr`], but returns `Ok(None)` rather than an error when
/// `name` is absent (used for optional attributes such as `state`/
/// `TargetMode`).
pub(crate) fn optional_attr(
    start: &BytesStart<'_>,
    path: &str,
    name: &str,
) -> Result<Option<String>, Error> {
    for attr in start.attributes() {
        let attr = attr.map_err(|err| Error::XmlParse {
            path: path.to_string(),
            source: Box::new(err),
        })?;
        if attr.key.as_ref() == name.as_bytes() {
            let value = attr
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map_err(|err| Error::XmlParse {
                    path: path.to_string(),
                    source: Box::new(err),
                })?;
            return Ok(Some(value.into_owned()));
        }
    }
    Ok(None)
}

/// Resolves an `Event::GeneralRef` (a `&#x...;`/`&#...;` character
/// reference, or one of the 5 predefined XML entities — the only entities
/// that can legally occur without a DTD, which `read_event` already
/// rejects) and appends the resolved text to `text`. Shared by
/// [`concat_rich_text`] and `parse/worksheet.rs`'s leaf-element text reader,
/// both of which need to reconstruct entity-bearing text content.
pub(crate) fn push_general_ref(
    text: &mut String,
    r: &quick_xml::events::BytesRef<'_>,
    path: &str,
) -> Result<(), Error> {
    match r.resolve_char_ref().map_err(|err| Error::XmlParse {
        path: path.to_string(),
        source: Box::new(err),
    })? {
        Some(ch) => text.push(ch),
        None => {
            let decoded = r.decode().map_err(|err| Error::XmlParse {
                path: path.to_string(),
                source: Box::new(err),
            })?;
            let resolved =
                quick_xml::escape::resolve_predefined_entity(&decoded).ok_or_else(|| {
                    Error::XmlParse {
                        path: path.to_string(),
                        source: format!("unknown XML entity reference: &{decoded};").into(),
                    }
                })?;
            text.push_str(resolved);
        }
    }
    Ok(())
}

/// Shared helper that extracts text-only content from the rich-text run
/// structure under `<si>` (shared strings) or `<is>` (inline strings) — a
/// sequence of `<r><t>...</t></r>` runs, or a single bare `<t>...</t>`.
/// `<t>` elements nested under `<rPr>` (per-run formatting) or `<rPh>`
/// (phonetic hints) are excluded from concatenation, since only their
/// *sibling* `<t>` (the run's actual text) contributes to the value.
///
/// Only text found *inside* a `<t>` element contributes to the result —
/// insignificant whitespace between sibling tags (`<is>`, `<r>`, `<rPr>`,
/// `</r>`, ...) in pretty-printed/indented XML is never captured, since it
/// arrives as its own `Event::Text` outside any `<t>` (Issue #53: this used
/// to be missed, since the previous implementation captured any
/// `Event::Text` seen anywhere under `<si>`/`<is>` with no positive check
/// for "currently inside `<t>`", so indentation whitespace leaked into the
/// result on non-minified XML).
///
/// Each `<t>`'s own content is trimmed of leading/trailing whitespace
/// unless that `<t>` carries `xml:space="preserve"` (Issue #56) — Excel's
/// own convention, and the one `xml:space`-conditional trimming other
/// Excel-compatible readers implement. Trimming is applied per `<t>`, not
/// to the whole concatenated result, since `xml:space` is an attribute of
/// the individual `<t>` element (ECMA-376), not of `<si>`/`<is>` as a whole.
///
/// Every fragment (`Event::Text`/`Event::CData`/`Event::GeneralRef`) is
/// appended directly to the final `text` buffer as it arrives — there is
/// no separate per-run buffer — and trimming is done in place on `text`'s
/// own tail once a `<t>` closes (see [`trim_tail_in_place`]). An earlier
/// version accumulated each run into its own `String` first specifically
/// to trim it before appending; benchmarking against a 50,000-entry shared
/// string table showed that extra allocation-and-copy costing roughly 17%
/// versus the pre-Issue-#56 baseline, which this in-place approach avoids
/// entirely while still trimming correctly, since nothing from a later run
/// is ever appended before the current run's own trim is resolved.
///
/// Called with the reader positioned just after the opening `<si>`/`<is>`
/// tag; consumes events up to and including the matching closing tag.
pub(crate) fn concat_rich_text<R: BufRead>(
    reader: &mut Reader<R>,
    path: &str,
) -> Result<String, Error> {
    let mut text = String::new();
    let mut buf = Vec::new();
    // Depth of exclusion zones (`<rPr>`/`<rPh>`) the cursor is currently
    // inside; `<t>` is only recognized while this is zero.
    let mut skip_depth: u32 = 0;
    // Byte offset into `text` where the `<t>` element currently being read
    // started contributing content — `None` whenever the cursor is not
    // inside a `<t>` element, which is when stray whitespace/text must be
    // ignored rather than appended.
    let mut t_start: Option<usize> = None;
    let mut t_preserve = false;

    loop {
        match read_event(reader, &mut buf, path)? {
            Event::Start(e) if e.local_name().as_ref() == b"rPr" => skip_depth += 1,
            Event::Start(e) if e.local_name().as_ref() == b"rPh" => skip_depth += 1,
            Event::End(e) if e.local_name().as_ref() == b"rPr" => skip_depth -= 1,
            Event::End(e) if e.local_name().as_ref() == b"rPh" => skip_depth -= 1,
            Event::Start(e) if skip_depth == 0 && e.local_name().as_ref() == b"t" => {
                t_preserve = optional_attr(&e, path, "xml:space")?.as_deref() == Some("preserve");
                t_start = Some(text.len());
            }
            Event::End(e) if skip_depth == 0 && e.local_name().as_ref() == b"t" => {
                if let Some(start) = t_start.take() {
                    if !t_preserve {
                        trim_tail_in_place(&mut text, start);
                    }
                }
            }
            Event::Text(e) if skip_depth == 0 && t_start.is_some() => {
                // Plain text content only: quick-xml 0.41 tokenizes any
                // `&...;` reference within content as a separate
                // `Event::GeneralRef`, never leaving escaped syntax inside
                // `Event::Text` for `.decode()` to unescape.
                let decoded = e.decode().map_err(|err| Error::XmlParse {
                    path: path.to_string(),
                    source: Box::new(err),
                })?;
                text.push_str(&normalize_line_endings(&decoded));
            }
            // `<t><![CDATA[...]]></t>` — a third-party-tool form real
            // Excel never writes but that legitimately occurs in the wild.
            // CDATA content is never XML-escaped by definition, so this
            // decodes it directly rather than going through `push_general_ref`.
            Event::CData(e) if skip_depth == 0 && t_start.is_some() => {
                let decoded = e.decode().map_err(|err| Error::XmlParse {
                    path: path.to_string(),
                    source: Box::new(err),
                })?;
                text.push_str(&normalize_line_endings(&decoded));
            }
            // `&#x...;`/`&#...;` (character references) or `&amp;`/`&lt;`/etc.
            // (the 5 predefined XML entities — the only ones that can occur
            // without a DTD, which `read_event` already rejects outright).
            Event::GeneralRef(e) if skip_depth == 0 && t_start.is_some() => {
                push_general_ref(&mut text, &e, path)?
            }
            Event::End(e)
                if e.local_name().as_ref() == b"si" || e.local_name().as_ref() == b"is" =>
            {
                break;
            }
            Event::Eof => {
                return Err(Error::MissingRequiredElement {
                    path: path.to_string(),
                    name: "si/is closing tag",
                })
            }
            _ => {}
        }
        buf.clear();
    }

    // Excel's own convention for embedding a literal CR that XML syntax
    // can't represent raw within a text node (Issue #57) — restore it
    // after concatenation rather than per-`<t>`, so the substitution is
    // correct even in the (unrealistic, but not worth special-casing
    // against) case where the literal marker straddles a run boundary.
    if text.contains("_x000D_") {
        text = text.replace("_x000D_", "\r");
    }

    Ok(text)
}

/// Trims leading/trailing whitespace from `text[start..]` in place, with no
/// additional allocation. Only ever called immediately after appending one
/// `<t>` run's content and before any later run's content has been
/// appended (`concat_rich_text` enforces this by construction — trimming
/// happens synchronously at each `</t>`), so removing leading whitespace
/// via [`String::drain`] only ever has to shift *this run's own* remaining
/// bytes, never the whole accumulated string.
fn trim_tail_in_place(text: &mut String, start: usize) {
    let trailing_len = text.len() - start - text[start..].trim_end().len();
    text.truncate(text.len() - trailing_len);

    let leading_len = text[start..].len() - text[start..].trim_start().len();
    if leading_len > 0 {
        text.drain(start..start + leading_len);
    }
}

/// XML 1.0 §2.11 End-of-Line Handling mandates that a parser normalize
/// every raw line break in the source document (a literal CRLF, or a lone
/// CR not followed by LF) to a single LF before the application ever sees
/// the text — different source files legitimately use different line-
/// ending conventions, and that difference must never leak into the
/// application's view of the actual character data. `quick-xml` does not
/// perform this normalization itself (verified against its source — only
/// XML *attribute*-value normalization is implemented), so this project's
/// own text-reading path does it explicitly. Discovered via Issue #57: the
/// fixture exercising the `_x000D_` escape happens to also use CRLF line
/// endings in its raw XML source, which — without this normalization —
/// doubled up into `\r\r\n` once combined with the escape's own `\r`.
///
/// Deliberately not applied to [`push_general_ref`]'s output: an explicit
/// `&#13;`/`&#x0D;` character reference is a real, intentional CR the
/// author chose to spell out as an entity rather than a raw line break, so
/// it must survive unnormalized (the XML spec's rule applies to raw source
/// bytes, before entity resolution, not to characters produced by it).
fn normalize_line_endings(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('\r') {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    std::borrow::Cow::Owned(out)
}

/// Reads the text content of a leaf element (`<v>...</v>`, `<f>...</f>`,
/// `<xdr:col>...</xdr:col>`) — no nested elements are expected — resolving
/// any `Event::GeneralRef` entities along the way. Called with the reader
/// positioned just after the element's opening tag; consumes events up to
/// and including its closing tag. Shared by `worksheet.rs` and
/// `drawing.rs`, the two modules whose leaf elements carry plain numeric/
/// text content rather than a nested run structure (contrast
/// `concat_rich_text`, which handles `<si>`/`<is>`'s richer `<r><t>` shape).
pub(crate) fn read_leaf_text(
    reader: &mut Reader<impl BufRead>,
    path: &str,
) -> Result<String, Error> {
    let mut text = String::new();
    let mut buf = Vec::new();
    loop {
        match read_event(reader, &mut buf, path)? {
            Event::Text(e) => {
                let decoded = e.decode().map_err(|err| Error::XmlParse {
                    path: path.to_string(),
                    source: Box::new(err),
                })?;
                text.push_str(&decoded);
            }
            Event::GeneralRef(e) => push_general_ref(&mut text, &e, path)?,
            Event::End(_) => break,
            Event::Eof => {
                return Err(Error::MissingRequiredElement {
                    path: path.to_string(),
                    name: "closing tag",
                })
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::sanitize::BoundedReader;
    use std::io::{self, Read};

    #[test]
    fn create_secure_reader_disables_text_trimming() {
        let xml = b"<root>  padded  </root>".as_slice();
        let mut reader = create_secure_reader(xml);
        let mut buf = Vec::new();

        // Advance past `<root>`.
        assert!(matches!(
            reader.read_event_into(&mut buf).unwrap(),
            Event::Start(_)
        ));
        buf.clear();
        match reader.read_event_into(&mut buf).unwrap() {
            Event::Text(t) => assert_eq!(t.decode().unwrap(), "  padded  "),
            other => panic!("expected Text event, got {other:?}"),
        }
    }

    #[test]
    fn convert_xml_error_maps_limit_exceeded_to_zip_bomb_detected() {
        let mut cumulative = 0u64;
        // `read_to_end` reads this single-chunk `&[u8]` source in one call,
        // so `actual` ends up as the full length read (11), not `limit + 1`
        // in general — keep `data` at exactly `limit + 1` bytes so the
        // expected `actual` stays simple and deterministic.
        let data = [0u8; 11];
        let mut bounded = BoundedReader::new(&data[..], 10, &mut cumulative, 1000);
        let mut out = Vec::new();
        let io_err = bounded.read_to_end(&mut out).unwrap_err();
        let quick_xml_err = quick_xml::Error::Io(std::sync::Arc::new(io_err));

        let err = convert_xml_error("xl/worksheets/sheet1.xml", quick_xml_err);
        match err {
            Error::ZipBombDetected { limit, actual } => {
                assert_eq!(limit, 10);
                assert_eq!(actual, 11);
            }
            other => panic!("expected ZipBombDetected, got {other:?}"),
        }
    }

    #[test]
    fn convert_xml_error_falls_back_to_xml_parse() {
        let io_err = io::Error::other("plain io failure");
        let quick_xml_err = quick_xml::Error::Io(std::sync::Arc::new(io_err));

        let err = convert_xml_error("xl/worksheets/sheet1.xml", quick_xml_err);
        match err {
            Error::XmlParse { path, .. } => assert_eq!(path, "xl/worksheets/sheet1.xml"),
            other => panic!("expected XmlParse, got {other:?}"),
        }
    }

    #[test]
    fn required_attr_returns_value_when_present() {
        let xml = br#"<c r="A1" t="s"></c>"#.as_slice();
        let mut reader = create_secure_reader(xml);
        let mut buf = Vec::new();
        let event = reader.read_event_into(&mut buf).unwrap();
        let Event::Start(start) = event else {
            panic!("expected Start event");
        };

        assert_eq!(required_attr(&start, "sheet1.xml", "r").unwrap(), "A1");
        assert_eq!(required_attr(&start, "sheet1.xml", "t").unwrap(), "s");
    }

    #[test]
    fn required_attr_errors_when_absent() {
        let xml = br#"<c r="A1"></c>"#.as_slice();
        let mut reader = create_secure_reader(xml);
        let mut buf = Vec::new();
        let event = reader.read_event_into(&mut buf).unwrap();
        let Event::Start(start) = event else {
            panic!("expected Start event");
        };

        let err = required_attr(&start, "sheet1.xml", "t").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement { name: "t", .. }
        ));
    }

    fn parse_si_body_result(xml: &[u8]) -> Result<String, Error> {
        let mut reader = create_secure_reader(xml);
        let mut buf = Vec::new();
        // Advance past the opening `<si>`/`<is>` tag.
        loop {
            match reader.read_event_into(&mut buf).unwrap() {
                Event::Start(e)
                    if e.local_name().as_ref() == b"si" || e.local_name().as_ref() == b"is" =>
                {
                    break
                }
                Event::Eof => panic!("no <si>/<is> start tag found"),
                _ => {}
            }
            buf.clear();
        }
        buf.clear();
        concat_rich_text(&mut reader, "xl/sharedStrings.xml")
    }

    fn parse_si_body(xml: &[u8]) -> String {
        parse_si_body_result(xml).unwrap()
    }

    #[test]
    fn concat_rich_text_single_bare_t() {
        let xml = b"<si><t>hello</t></si>";
        assert_eq!(parse_si_body(xml), "hello");
    }

    #[test]
    fn concat_rich_text_multiple_runs() {
        // Real Excel output marks a run's meaningful trailing space with
        // xml:space="preserve" (Issue #56) — without it, that space would
        // now correctly be trimmed away rather than surviving by accident.
        let xml = br#"<si><r><t xml:space="preserve">hello </t></r><r><t>world</t></r></si>"#;
        assert_eq!(parse_si_body(xml), "hello world");
    }

    #[test]
    fn concat_rich_text_excludes_rpr_and_rph() {
        let xml = b"<si><r><rPr><b/></rPr><t>bold</t></r><rPh><t>phonetic</t></rPh></si>";
        assert_eq!(parse_si_body(xml), "bold");
    }

    #[test]
    fn concat_rich_text_ignores_whitespace_between_sibling_tags_in_pretty_printed_xml() {
        // Issue #53: indentation between <r>/<rPr>/<t> in non-minified XML
        // must never leak into the result — only text found inside <t>
        // itself contributes.
        let xml = b"<si>\n  <r>\n    <rPr>\n      <b/>\n    </rPr>\n    <t>NN</t>\n  </r>\n</si>";
        assert_eq!(parse_si_body(xml), "NN");
    }

    #[test]
    fn concat_rich_text_trims_whitespace_unless_xml_space_preserve() {
        // Issue #56: Excel/calamine convention — trim leading/trailing
        // whitespace from <t> content unless xml:space="preserve".
        let untagged = b"<si><t>  trimmed value  </t></si>";
        assert_eq!(parse_si_body(untagged), "trimmed value");

        let preserved = br#"<si><t xml:space="preserve">  preserved value  </t></si>"#;
        assert_eq!(parse_si_body(preserved), "  preserved value  ");

        let whitespace_only = b"<si><t> \t\n</t></si>";
        assert_eq!(parse_si_body(whitespace_only), "");
    }

    #[test]
    fn concat_rich_text_trims_each_run_independently() {
        // Trimming is scoped to the individual <t> (xml:space is its own
        // attribute per ECMA-376), not to the whole concatenated result —
        // an untrimmed run's boundary whitespace must not bleed into an
        // adjacent trimmed run.
        let xml = br#"<si><r><t xml:space="preserve">  a  </t></r><r><t>  b  </t></r></si>"#;
        assert_eq!(parse_si_body(xml), "  a  b");
    }

    #[test]
    fn concat_rich_text_restores_x000d_escape() {
        // Issue #57: Excel's own convention for embedding a literal CR
        // that XML can't represent raw within a text node.
        let xml = b"<si><t>ABC_x000D_\nDEF</t></si>";
        assert_eq!(parse_si_body(xml), "ABC\r\nDEF");
    }

    #[test]
    fn concat_rich_text_x000d_escape_followed_by_raw_crlf_source_line_ending_is_not_doubled() {
        // Regression test discovered while verifying Issue #57 against a
        // real calamine fixture: when the raw XML source itself uses CRLF
        // line endings, the literal \r\n right after the _x000D_ marker
        // must first be normalized to a single \n (XML 1.0 end-of-line
        // handling) before the marker's own \r is restored — otherwise the
        // result doubles up into "\r\r\n" instead of "\r\n".
        let xml = b"<si><t>ABC_x000D_\r\nDEF</t></si>";
        assert_eq!(parse_si_body(xml), "ABC\r\nDEF");
    }

    #[test]
    fn concat_rich_text_normalizes_raw_crlf_and_lone_cr_to_lf() {
        let crlf = b"<si><t xml:space=\"preserve\">a\r\nb</t></si>";
        assert_eq!(parse_si_body(crlf), "a\nb");

        let lone_cr = b"<si><t xml:space=\"preserve\">a\rb</t></si>";
        assert_eq!(parse_si_body(lone_cr), "a\nb");
    }

    #[test]
    fn concat_rich_text_does_not_normalize_an_explicit_cr_character_reference() {
        // An explicit &#13; is a deliberate, author-chosen CR — distinct
        // from a raw source line break — and must survive unnormalized.
        let xml = b"<si><t xml:space=\"preserve\">a&#13;b</t></si>";
        assert_eq!(parse_si_body(xml), "a\rb");
    }

    #[test]
    fn concat_rich_text_reads_cdata_content() {
        // A third-party-tool form real Excel never writes, but occurs in
        // the wild — previously silently ignored (fell into the catch-all
        // branch), producing an empty string instead of the CDATA content.
        let xml = b"<si><t><![CDATA[Hello CDATA]]></t></si>";
        assert_eq!(parse_si_body(xml), "Hello CDATA");
    }

    #[test]
    fn concat_rich_text_invalid_utf8_in_plain_text_is_xml_parse_error() {
        // Same as the CDATA case above, but for the plain-text decode path
        // (unchanged by this PR, but exercised through the new `t_start`
        // gating for the first time here).
        let mut xml = b"<si><t>".to_vec();
        xml.push(0xFF);
        xml.extend_from_slice(b"</t></si>");
        let err = parse_si_body_result(&xml).unwrap_err();
        assert!(matches!(err, Error::XmlParse { .. }));
    }

    #[test]
    fn concat_rich_text_invalid_utf8_in_cdata_is_xml_parse_error() {
        // Regression test for the CData decode error path introduced
        // alongside CDATA support: an invalid byte sequence must surface
        // as Error::XmlParse, not panic.
        let mut xml = b"<si><t><![CDATA[".to_vec();
        xml.push(0xFF); // not valid UTF-8 on its own
        xml.extend_from_slice(b"]]></t></si>");
        let err = parse_si_body_result(&xml).unwrap_err();
        assert!(matches!(err, Error::XmlParse { .. }));
    }

    #[test]
    fn concat_rich_text_eof_before_closing_tag_is_missing_required_element() {
        let xml = b"<si><t>unterminated";
        let err = parse_si_body_result(xml).unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement {
                name: "si/is closing tag",
                ..
            }
        ));
    }

    fn parse_leaf_text_result(xml: &[u8]) -> Result<String, Error> {
        let mut reader = create_secure_reader(xml);
        let mut buf = Vec::new();
        // Advance past the opening <v> tag.
        loop {
            match reader.read_event_into(&mut buf).unwrap() {
                Event::Start(e) if e.local_name().as_ref() == b"v" => break,
                Event::Eof => panic!("no <v> start tag found"),
                _ => {}
            }
            buf.clear();
        }
        buf.clear();
        read_leaf_text(&mut reader, "xl/worksheets/sheet1.xml")
    }

    #[test]
    fn read_leaf_text_resolves_a_general_ref_entity() {
        // &#38; is the numeric character reference for '&', tokenized by
        // quick-xml as Event::GeneralRef rather than folded into
        // Event::Text.
        let text = parse_leaf_text_result(b"<v>4&#38;2</v>").unwrap();
        assert_eq!(text, "4&2");
    }

    #[test]
    fn read_leaf_text_eof_before_closing_tag_is_missing_required_element() {
        let err = parse_leaf_text_result(b"<v>unterminated").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement {
                name: "closing tag",
                ..
            }
        ));
    }

    #[test]
    fn read_leaf_text_ignores_a_comment_and_still_captures_surrounding_text() {
        let text = parse_leaf_text_result(b"<v>4<!-- note -->2</v>").unwrap();
        assert_eq!(text, "42");
    }

    #[test]
    fn read_event_rejects_doctype_and_stops_reading() {
        let xml =
            br#"<!DOCTYPE foo [ <!ENTITY xxe SYSTEM "file:///etc/passwd"> ]><root/>"#.as_slice();
        let mut reader = create_secure_reader(xml);
        let mut buf = Vec::new();

        let err = read_event(&mut reader, &mut buf, "xl/sharedStrings.xml").unwrap_err();
        assert!(matches!(err, Error::DoctypeRejected { .. }));
    }

    #[test]
    fn read_event_passes_through_legitimate_xml_without_false_positives() {
        let xml = br#"<?xml version="1.0"?><sst><si><t>ok</t></si></sst>"#.as_slice();
        let mut reader = create_secure_reader(xml);
        let mut buf = Vec::new();

        loop {
            if read_event(&mut reader, &mut buf, "xl/sharedStrings.xml").unwrap() == Event::Eof {
                break;
            }
            buf.clear();
        }
    }

    #[test]
    fn read_event_converts_syntax_error_via_convert_xml_error() {
        let xml = b"<root><unclosed></root>".as_slice();
        let mut reader = create_secure_reader(xml);
        let mut buf = Vec::new();

        let mut last_err = None;
        loop {
            buf.clear();
            match read_event(&mut reader, &mut buf, "sheet1.xml") {
                Ok(Event::Eof) => break,
                Ok(_) => continue,
                Err(err) => {
                    last_err = Some(err);
                    break;
                }
            }
        }
        assert!(matches!(last_err, Some(Error::XmlParse { .. })));
    }

    #[test]
    fn read_event_converts_bounded_reader_limit_via_convert_xml_error() {
        let mut cumulative = 0u64;
        let data = b"<root>this is too much data</root>".to_vec();
        let bounded = BoundedReader::new(&data[..], 5, &mut cumulative, 1000);
        let mut reader = create_secure_reader(io::BufReader::new(bounded));
        let mut buf = Vec::new();

        let err = loop {
            buf.clear();
            match read_event(&mut reader, &mut buf, "sheet1.xml") {
                Ok(Event::Eof) => panic!("expected an error before EOF"),
                Ok(_) => continue,
                Err(err) => break err,
            }
        };
        assert!(matches!(err, Error::ZipBombDetected { limit: 5, .. }));
    }
}
