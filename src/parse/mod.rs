//! XML parsing layer: consolidates the `quick-xml` dependency. Secure
//! `Reader` construction, XML error conversion, and helpers shared by every
//! submodule live here; each submodule interprets one OOXML part's
//! structure.

mod relationships;

#[allow(unused_imports)]
pub(crate) use relationships::{parse_relationships, Relationship, RelationshipMap, TargetMode};

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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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

/// Shared helper that extracts text-only content from the rich-text run
/// structure under `<si>` (shared strings) or `<is>` (inline strings) — a
/// sequence of `<r><t>...</t></r>` runs, or a single bare `<t>...</t>`.
/// `<t>` elements nested under `<rPr>` (per-run formatting) or `<rPh>`
/// (phonetic hints) are excluded from concatenation, since only their
/// *sibling* `<t>` (the run's actual text) contributes to the value.
///
/// Called with the reader positioned just after the opening `<si>`/`<is>`
/// tag; consumes events up to and including the matching closing tag.
#[allow(dead_code)]
pub(crate) fn concat_rich_text<R: BufRead>(
    reader: &mut Reader<R>,
    path: &str,
) -> Result<String, Error> {
    let mut text = String::new();
    let mut buf = Vec::new();
    // Depth of exclusion zones (`<rPr>`/`<rPh>`) the cursor is currently
    // inside; `<t>` is only appended to `text` while this is zero.
    let mut skip_depth: u32 = 0;

    loop {
        match read_event(reader, &mut buf, path)? {
            Event::Start(e) if e.local_name().as_ref() == b"rPr" => skip_depth += 1,
            Event::Start(e) if e.local_name().as_ref() == b"rPh" => skip_depth += 1,
            Event::End(e) if e.local_name().as_ref() == b"rPr" => skip_depth -= 1,
            Event::End(e) if e.local_name().as_ref() == b"rPh" => skip_depth -= 1,
            Event::Text(e) if skip_depth == 0 => {
                let decoded = e.decode().map_err(|err| Error::XmlParse {
                    path: path.to_string(),
                    source: Box::new(err),
                })?;
                let unescaped =
                    quick_xml::escape::unescape(&decoded).map_err(|err| Error::XmlParse {
                        path: path.to_string(),
                        source: Box::new(err),
                    })?;
                text.push_str(&unescaped);
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

    fn parse_si_body(xml: &[u8]) -> String {
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
        concat_rich_text(&mut reader, "xl/sharedStrings.xml").unwrap()
    }

    #[test]
    fn concat_rich_text_single_bare_t() {
        let xml = b"<si><t>hello</t></si>";
        assert_eq!(parse_si_body(xml), "hello");
    }

    #[test]
    fn concat_rich_text_multiple_runs() {
        let xml = b"<si><r><t>hello </t></r><r><t>world</t></r></si>";
        assert_eq!(parse_si_body(xml), "hello world");
    }

    #[test]
    fn concat_rich_text_excludes_rpr_and_rph() {
        let xml = b"<si><r><rPr><b/></rPr><t>bold</t></r><rPh><t>phonetic</t></rPh></si>";
        assert_eq!(parse_si_body(xml), "bold");
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
