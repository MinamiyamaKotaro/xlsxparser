# `parse/mod.rs` Design Doc

*[日本語](mod.md)*

Design doc for `src/parse/mod.rs`. Per [architecture.md](../architecture.en.md), `parse/` is "the layer that consolidates dependence on XML-parsing libraries such as `quick-xml`." This file gathers the cross-cutting concerns shared by the individual parsers ([relationships.md](relationships.en.md) / [workbook.md](workbook.en.md) / [shared_strings.md](shared_strings.en.md) / [styles.md](styles.en.md) / [worksheet.md](worksheet.en.md)): secure `Reader` construction, XML parse-error conversion, and shared helpers for attribute lookup and rich-text concatenation. The conversion logic that [container/sanitize.md Error Handling Policy](../container/sanitize.en.md) said would be "co-located with `parse/mod.rs`'s secure Reader factory" (`convert_xml_error`) is finalized here.

## Responsibility / Scope

- Declares submodules (`mod relationships; mod workbook; mod shared_strings; mod styles; mod worksheet; mod theme;`) and re-exports crate-internal types
- Provides `create_secure_reader`, the sole gateway for constructing a `quick_xml::Reader` with XXE mitigations already applied. Every module under `parse/` must obtain its `Reader` only through this function, never by calling `Reader::from_reader` directly (this implements architecture.md's rationale: "if each parser initializes its own `Reader`, there is a risk of a missed configuration")
- Provides `convert_xml_error`, the sole gateway for converting `quick_xml::Error` into [`crate::error::Error`](../error.en.md). This is also where limit-exceeded errors from [container/sanitize.md](../container/sanitize.en.md)'s `BoundedReader` (Zip Bomb protection) are detected and converted into `Error::ZipBombDetected`
- Provides `read_event`, the sole gateway for reading events. `quick-xml` is a non-validating parser that never resolves a DTD internal subset or an external entity even in its default configuration, so classic XXE cannot occur in the first place — but rather than resting on that assumption alone, this function actively rejects any `<!DOCTYPE ...>` declaration (`Event::DocType`) unconditionally the moment it is detected as XML syntax (fail closed), giving XXE mitigation an explicit, verifiable form that doesn't depend on the parser's internal implementation or future version changes. Every module under `parse/` reads events only through this function, never calling `Reader::read_event_into` directly (reflects [the security review](../../security/design-review.en.md) Finding 1)
- Provides small shared helpers for patterns that would otherwise be duplicated across modules: required-attribute lookup (returning `Error::MissingRequiredElement` when absent) and concatenating rich-text runs (`<r><t>...</t></r>`) used by both shared strings and inline strings
- **Not responsible for**: interpreting the structure specific to any individual XML part (`_rels` / `workbook.xml` / `sharedStrings.xml` / `styles.xml` / `sheetX.xml` — each submodule's job), semantic validation/resolution of parsed results (`resolve/`)

## Key Types / Functions (draft)

```rust
mod relationships;
mod workbook;
mod shared_strings;
mod styles;
mod worksheet;
mod theme;

pub(crate) use relationships::{Relationship, RelationshipMap, TargetMode, parse_relationships};
pub(crate) use shared_strings::{SharedStringTable, parse_shared_strings};
pub(crate) use styles::parse_styles;
pub(crate) use workbook::{WorkbookSheetEntry, parse_workbook_xml};
pub(crate) use worksheet::{PendingSharedString, PendingStyle, WorksheetParseOutput, parse_worksheet};
pub(crate) use theme::parse_theme;

use crate::error::Error;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::borrow::Cow;
use std::io::BufRead;

/// The sole gateway for constructing a `Reader` with XXE mitigations applied.
///
/// `quick-xml` is a non-validating parser that, even in its default
/// configuration, never fetches external entities or an external DTD
/// subset — so classic XXE (local file disclosure, SSRF) cannot occur in the
/// first place. Even so, requirements chapter 2's requirement to "disable
/// external entity expansion during XML parsing" should not remain an
/// implicit assumption; this function exists to make that setting explicit
/// (whether the crate exposes an API that explicitly disables it depends on
/// the version selected — see Open Question 1).
///
/// Sets `trim_text(false)` so element text is never auto-trimmed. This is
/// the default needed to not lose shared strings' `xml:space="preserve"`
/// (see [shared_strings.md](shared_strings.en.md)); whether to actually
/// preserve whitespace is decided per submodule.
pub(crate) fn create_secure_reader<R: BufRead>(inner: R) -> Reader<R> {
    let mut reader = Reader::from_reader(inner);
    reader.config_mut().trim_text(false);
    reader
}

/// The sole gateway for converting `quick_xml::Error` into
/// `crate::error::Error`. Per [container/sanitize.md Error Handling
/// Policy](../container/sanitize.en.md), a limit-exceeded error from the
/// `BoundedReader` (Zip Bomb protection) propagates up wrapped as an
/// `io::Error` inside `quick_xml::Error::Io`, so this first downcasts to
/// `container::sanitize::LimitExceeded` and, if it matches, returns
/// `Error::ZipBombDetected`. Otherwise it wraps the error as
/// `Error::XmlParse`, type-erased per [error.md](../error.en.md)'s policy of
/// never exposing `quick_xml::Error` directly in the public API.
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
/// converts any error via `convert_xml_error`, and — if the returned `Event`
/// is `Event::DocType` (a `<!DOCTYPE ...>` declaration) — returns
/// `Error::DoctypeRejected` unconditionally without interpreting its content
/// at all (fail closed).
///
/// None of OOXML's `_rels`/`workbook.xml`/`sharedStrings.xml`/`styles.xml`/
/// `sheetX.xml` parts ever carry a DOCTYPE declaration per spec, so this
/// check never rejects a legitimate `.xlsx`. The design assumption that
/// `quick-xml` itself is a non-validating parser that resolves neither a
/// DTD internal subset nor an external entity, so classic XXE cannot occur
/// even in the default configuration (see Responsibility / Scope), still
/// holds — but this function acts as an independent layer of defense that
/// keeps working even if that assumption were ever broken by a future
/// version change or a switch to a different parser, by cutting processing
/// off the moment a DOCTYPE declaration's mere presence is detected at the
/// XML syntax level. Every module under `parse/` reads events only through
/// this function, never calling `reader.read_event_into` directly.
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

/// Reads attribute `name` from `start`. Returns `Error::MissingRequiredElement`
/// if it is absent. Fully decoded and unescaped (see Open Question 1 for the
/// `Attribute::normalized_value` API this ended up using).
pub(crate) fn required_attr(
    start: &BytesStart<'_>,
    path: &str,
    name: &'static str,
) -> Result<String, Error> {
    for attr in start.attributes() {
        let attr = attr.map_err(|err| Error::XmlParse { path: path.to_string(), source: Box::new(err) })?;
        if attr.key.as_ref() == name.as_bytes() {
            let value = attr
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map_err(|err| Error::XmlParse { path: path.to_string(), source: Box::new(err) })?;
            return Ok(value.into_owned());
        }
    }
    Err(Error::MissingRequiredElement { path: path.to_string(), name })
}

/// Shared helper that extracts text-only content from the rich-text run
/// structure under `<si>` (shared strings) or `<is>` (inline strings) — a
/// sequence of `<r><t>...</t></r>` runs, or a single bare `<t>...</t>`.
/// `<t>` elements nested under `<rPr>` (per-run formatting) or `<rPh>`
/// (phonetic hints; see [shared_strings.md](shared_strings.en.md)) are
/// excluded from concatenation. Both [shared_strings.md](shared_strings.en.md)
/// and [worksheet.md](worksheet.en.md) (`t="inlineStr"` cells) need to
/// interpret this exact same structure (OOXML defines the run layout of
/// `<si>`/`<is>` identically), so it is centralized here to avoid
/// duplicating the logic.
///
/// Takes `path` (for error context) rather than a caller-owned `buf` like
/// the draft did: this function reads a variable, unbounded number of
/// events up to the closing `</si>`/`</is>` tag, so it manages its own
/// internal buffer rather than threading one through from the caller
/// (finalized at implementation time). Called with the reader already
/// positioned just after the opening `<si>`/`<is>` tag.
///
/// **Post-implementation revisions (Issues #53/#56/#57)**: the version
/// below is significantly different from what first shipped. Only text
/// found *inside* a `<t>` element contributes to the result — the original
/// implementation captured any `Event::Text` seen anywhere under
/// `<si>`/`<is>` with no positive "currently inside `<t>`" check, so
/// indentation whitespace between sibling tags in pretty-printed/indented
/// XML leaked into the result (Issue #53). Each `<t>`'s own content is now
/// also trimmed of leading/trailing whitespace unless that `<t>` carries
/// `xml:space="preserve"` (Issue #56, matching the convention Excel and
/// other Excel-compatible readers follow — `xml:space` is an attribute of
/// the individual `<t>` element, not of `<si>`/`<is>` as a whole, so
/// trimming is scoped per run). Excel's `_x000D_` escape for a literal CR
/// that XML syntax can't represent raw within a text node is restored
/// after concatenation (Issue #57).
///
/// Every fragment (`Event::Text`/`Event::CData`/`Event::GeneralRef`) is
/// appended directly to the final `text` buffer as it arrives, with no
/// separate per-run buffer; trimming happens in place on `text`'s own tail
/// once a `<t>` closes (see `trim_tail_in_place`). An interim
/// implementation accumulated each run into its own `String` specifically
/// so it could trim before appending, but benchmarking against a
/// 50,000-entry shared string table showed that extra allocation-and-copy
/// costing roughly 17% versus the pre-Issue-#56 baseline; this in-place
/// approach avoids the allocation entirely (dropping the regression to
/// roughly 10%) while trimming correctly, since nothing from a later run
/// is ever appended before the current run's own trim is resolved.
///
/// Also decodes `Event::CData` (`<t><![CDATA[...]]></t>`) the same way as
/// `Event::Text` — a form real Excel never writes but that third-party
/// tools legitimately produce, previously silently dropped since it fell
/// into the `_ => {}` catch-all.
///
/// Every raw `Event::Text`/`Event::CData` fragment is also run through
/// `normalize_line_endings` before being appended: `quick-xml` does not
/// implement XML 1.0 §2.11's mandatory end-of-line normalization (a raw
/// CRLF or lone CR in the source must become a single LF before the
/// application ever sees it), so this project's own text-reading path does
/// it explicitly — discovered because the fixture exercising `_x000D_`
/// happens to also use CRLF line endings in its raw XML source, which
/// without this normalization doubled up into `\r\r\n`. Deliberately not
/// applied to `push_general_ref`'s output — an explicit `&#13;` character
/// reference is a real, intentional CR the author chose to spell out, not
/// a raw source line break, and must survive unnormalized.
pub(crate) fn concat_rich_text<R: BufRead>(
    reader: &mut Reader<R>,
    path: &str,
) -> Result<String, Error> {
    let mut text = String::new();
    let mut buf = Vec::new();
    let mut skip_depth: u32 = 0; // inside <rPr>/<rPh>?
    // Byte offset into `text` where the `<t>` element currently being read
    // started contributing content — `None` outside any `<t>`.
    let mut t_start: Option<usize> = None;
    let mut t_preserve = false;
    loop {
        match read_event(reader, &mut buf, path)? {
            Event::Start(e) if e.local_name().as_ref() == b"rPr" || e.local_name().as_ref() == b"rPh" => skip_depth += 1,
            Event::End(e) if e.local_name().as_ref() == b"rPr" || e.local_name().as_ref() == b"rPh" => skip_depth -= 1,
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
                let decoded = e.decode().map_err(|err| Error::XmlParse { path: path.to_string(), source: Box::new(err) })?;
                text.push_str(&normalize_line_endings(&decoded));
            }
            Event::CData(e) if skip_depth == 0 && t_start.is_some() => {
                let decoded = e.decode().map_err(|err| Error::XmlParse { path: path.to_string(), source: Box::new(err) })?;
                text.push_str(&normalize_line_endings(&decoded));
            }
            // `&#x...;`/`&#...;` or one of the 5 predefined XML entities
            // (`&amp;`/`&lt;`/`&gt;`/`&apos;`/`&quot;`) — the only entities
            // that can legally appear without a DTD, which `read_event`
            // already rejects.
            Event::GeneralRef(e) if skip_depth == 0 && t_start.is_some() => {
                push_general_ref(&mut text, &e, path)?
            }
            Event::End(e) if e.local_name().as_ref() == b"si" || e.local_name().as_ref() == b"is" => break,
            Event::Eof => return Err(Error::MissingRequiredElement { path: path.to_string(), name: "si/is closing tag" }),
            _ => {}
        }
        buf.clear();
    }
    // Restored once at the end rather than per-run, so it stays correct
    // even in the (unrealistic) case where the marker straddles a run boundary.
    if text.contains("_x000D_") {
        text = text.replace("_x000D_", "\r");
    }
    Ok(text)
}

/// Trims leading/trailing whitespace from `text[start..]` in place, with no
/// additional allocation — only ever called immediately after appending one
/// `<t>` run's content and before any later run's content has been
/// appended, so removing leading whitespace via `String::drain` only ever
/// shifts *this run's own* remaining bytes, never the whole accumulated
/// string.
fn trim_tail_in_place(text: &mut String, start: usize) {
    let trailing_len = text.len() - start - text[start..].trim_end().len();
    text.truncate(text.len() - trailing_len);
    let leading_len = text[start..].len() - text[start..].trim_start().len();
    if leading_len > 0 {
        text.drain(start..start + leading_len);
    }
}
```

## Dependencies

- Depends on: [`container/sanitize.rs`](../container/sanitize.en.md) (only for downcasting to `LimitExceeded` — no dependency on `container::ZipContainer` itself; architecture.md design policy 3 forbids `container` and `parse` from knowing about each other's orchestration role directly, and referencing this single internal error type does not violate that), [`error.rs`](../error.en.md), and the external `quick-xml` crate
- Depended on by: every submodule under `parse/` ([relationships.rs](relationships.en.md) / [workbook.rs](workbook.en.md) / [shared_strings.rs](shared_strings.en.md) / [styles.rs](styles.en.md) / [worksheet.rs](worksheet.en.md) / [theme.rs](theme.en.md)), `pipeline.rs` (calls the re-exported parse functions)

`convert_xml_error`'s reference to `container::sanitize::LimitExceeded` is simply the implementation of what both [container/sanitize.md Error Handling Policy](../container/sanitize.en.md) and [container/mod.md Error Handling Policy](../container/mod.en.md) had already settled — that the conversion boundary lives where `parse/` converts `quick_xml::Error` into `crate::error::Error`. This resolves the open point both of those files had left pending.

`read_event` returning `Error::DoctypeRejected` ([newly added](../error.en.md)) makes it a third "sole gateway," alongside `create_secure_reader` and `convert_xml_error`. It doubles up XXE mitigation as an active check performed on every event actually read, rather than leaving it solely to the passive mechanism of how the `Reader` is configured at construction time (reflects [the security review](../../security/design-review.en.md) Finding 1).

## Error Handling Policy

- `create_secure_reader` cannot fail (constructing a `Reader` never fails by itself; I/O errors on the underlying stream only surface when `read_event` is actually called)
- `convert_xml_error` always converts any `quick_xml::Error` into some variant of `crate::error::Error` (never panics). Anything that doesn't match `Error::ZipBombDetected` always falls back to `Error::XmlParse`, so no unknown variant is silently swallowed
- When `read_event` detects `Event::DocType`, it returns `Error::DoctypeRejected` immediately without interpreting the declaration's content at all (e.g. whether its internal subset defines an entity). Rather than an allowlist-style judgment that "only rejects it if it contains a suspicious entity definition," treating the mere presence of a DOCTYPE declaration as grounds for rejection structurally eliminates any chance of a detection gap caused by a mistake in parsing the entity-definition syntax (fail closed — the same principle [container/sanitize.md](../container/sanitize.en.md)'s `validate_entry_path` follows: "ambiguous or uninterpretable input errs on the side of rejection")
- `required_attr` returns `Result` rather than panicking, since a missing attribute can originate from untrusted external input (a malformed `.xlsx`)

## Testing Strategy

- Verify `create_secure_reader` produces a `Reader` with the expected configuration (`trim_text(false)`, etc.)
- `convert_xml_error`: verify that a `quick_xml::Error::Io` wrapping the `io::Error` produced by `BoundedReader`'s `LimitExceeded` converts correctly to `Error::ZipBombDetected`, preserving `limit`/`actual`
- `convert_xml_error`: verify that an ordinary XML syntax error (e.g. an unclosed tag) converts to `Error::XmlParse` with `path` set correctly
- `required_attr`: verify it retrieves the value when the attribute is present, and returns `Error::MissingRequiredElement` when absent
- `concat_rich_text`: verify a single `<t>`, multiple `<r><t>` runs, and input containing `<rPh>` each produce the expected string (exhaustive cases live in [shared_strings.md Testing Strategy](shared_strings.en.md); this file only verifies the wiring)
- **`concat_rich_text`: verify indentation whitespace between sibling tags in pretty-printed XML is never captured** (Issue #53), **that each `<t>`'s own content is trimmed unless it carries `xml:space="preserve"`, applied per run rather than to the whole concatenated result** (Issue #56), **that a literal `_x000D_` marker is restored to an actual CR** (Issue #57), **that `<t><![CDATA[...]]></t>` content is read**, and **that a raw CRLF/lone-CR line ending in the source is normalized to a single LF, while an explicit `&#13;` character reference is left unnormalized**
- **`read_event`: verify that feeding a malicious XML payload containing a DOCTYPE declaration and an external entity reference (e.g. `<!DOCTYPE foo [ <!ENTITY xxe SYSTEM "file:///etc/passwd"> ]>`) returns `Error::DoctypeRejected` the moment `Event::DocType` is detected, and reads no further events** (verifies requirements chapter 2's XXE requirement itself; a regression test for the explicit, verifiable mitigation [the security review](../../security/design-review.en.md) Finding 1 called for, instead of resting on an implicit assumption alone)
- `read_event`: verify that legitimate XML with no external entity reference and no DOCTYPE declaration (representative of `_rels`/`workbook.xml`/`sharedStrings.xml`/`styles.xml`/`sheetX.xml`) returns events as normal, and `Error::DoctypeRejected` is never raised spuriously (a regression test guarding against false positives on a legitimate `.xlsx`)
- `read_event`: verify that XML with no DOCTYPE but a syntax error, and input that exceeds `BoundedReader`'s limit, each convert correctly to `Error::XmlParse`/`Error::ZipBombDetected` via `convert_xml_error` (a wiring test confirming error conversion happens before the `Event::DocType` check)
- None of the above XXE-related tests are repeated in individual `parse/*.rs` files (per the [PR #9 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204), they are centralized here, where `read_event` is defined)

## Open Questions

1. ~~Finalizing the quick-xml version and `Reader` configuration API~~ → **Resolved**: quick-xml 0.41. `Reader::config_mut().trim_text(false)` matches the draft exactly. Two API changes from the draft, both surfaced by the compiler as deprecation/missing-method errors rather than silent behavior differences:
   - `Attribute::unescape_value()` is deprecated in favor of `Attribute::normalized_value(XmlVersion::Implicit1_0)`, used by `required_attr`. It performs the same decode+entity-unescape+AttValue-normalization as the old method.
   - `BytesText` no longer has an `unescape()` method at all. More importantly, in 0.41 a `&...;` reference (character reference or the 5 predefined XML entities) is no longer embedded inside the surrounding `Event::Text`'s raw content for a caller to unescape — the tokenizer splits it out as its own interleaved `Event::GeneralRef(BytesRef)` event. `concat_rich_text` (the only caller that needs to reconstruct entity-bearing text) therefore handles both: `Event::Text` content only ever needs `.decode()` (no entities left to unescape), and `Event::GeneralRef` is resolved via `BytesRef::resolve_char_ref()` (numeric refs) falling back to `quick_xml::escape::resolve_predefined_entity()` (named refs — the only ones that can legally occur, since `read_event` already rejects any DOCTYPE that could define a custom entity). Discovered by a failing test (`rPh` furigana case using `&#x...;` numeric escapes) rather than a compiler error, since `Event::Text` still compiled and ran, just silently omitted the referenced characters.

   `read_event`'s XXE mitigation (unconditionally rejecting `Event::DocType`) was unaffected by either change, confirming the independence this open question anticipated (reflects [the security review](../../security/design-review.en.md) Finding 1): the `Event` enum's shape (the existence of the `DocType` variant) stayed stable across the version bump.
2. ~~Where the XXE-non-applicability test lives~~ → **Resolved**: centralized in this file's unit tests (reflects the [PR #9 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204)). Placing it alongside where `read_event` is defined lets it directly verify the core of the mitigation — that `Event::DocType` is reliably detected and rejected — without repeating it in individual `parse/*.rs` files.
3. ~~`required_attr`'s return type~~ → **Resolved for now**: kept as an allocated `String` (not `Cow<str>`/`&str`) for simplicity; `Attribute::normalized_value` already returns `Cow<str>` internally; revisit if profiling shows attribute allocation is a hot path.
4. ~~How to resolve namespaces (e.g. `r:id`)~~ → **Resolved**: does not adopt `quick_xml::NsReader`'s namespace-URI-based resolution; simplifies to plain string-prefix matching (matching against an attribute name that includes the prefix, e.g. `"r:id"`, when calling `required_attr`) — reflects the [PR #9 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204). This prioritizes requirements chapter 1's "lightweight and fast" policy, given that every major producer (Excel, Google Sheets, LibreOffice, Apache POI, etc.) uses `r` as the relationships-namespace prefix without exception in practice. Even in the very rare case of a legally-formed document that declares the namespace under a different alias, the attribute simply comes back "not found," failing closed as `Error::MissingRequiredElement` rather than silently reading a wrong value.
5. **`Reader` internal buffer sizing for large streams such as `worksheet.xml`**: quick-xml grows its buffer dynamically by default, but there is room to explicitly tune the initial buffer size for the "grid-paper Excel" sheet sizes the requirements target. To be settled during [worksheet.md](worksheet.en.md)'s design/implementation based on profiling.
