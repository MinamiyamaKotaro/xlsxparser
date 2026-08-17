# `parse/mod.rs` Design Doc

*[日本語](mod.md)*

Design doc for `src/parse/mod.rs`. Per [architecture.md](../architecture.en.md), `parse/` is "the layer that consolidates dependence on XML-parsing libraries such as `quick-xml`." This file gathers the cross-cutting concerns shared by the individual parsers ([relationships.md](relationships.en.md) / [workbook.md](workbook.en.md) / [shared_strings.md](shared_strings.en.md) / [styles.md](styles.en.md) / [worksheet.md](worksheet.en.md)): secure `Reader` construction, XML parse-error conversion, and shared helpers for attribute lookup and rich-text concatenation. The conversion logic that [container/sanitize.md Error Handling Policy](../container/sanitize.en.md) said would be "co-located with `parse/mod.rs`'s secure Reader factory" (`convert_xml_error`) is finalized here.

## Responsibility / Scope

- Declares submodules (`mod relationships; mod workbook; mod shared_strings; mod styles; mod worksheet;`) and re-exports crate-internal types
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

pub(crate) use relationships::{Relationship, RelationshipMap, TargetMode, parse_relationships};
pub(crate) use shared_strings::{SharedStringTable, parse_shared_strings};
pub(crate) use styles::parse_styles;
pub(crate) use workbook::{WorkbookSheetEntry, parse_workbook_xml};
pub(crate) use worksheet::{PendingSharedString, PendingStyle, WorksheetParseOutput, parse_worksheet};

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
/// if it is absent. Whether the returned string is fully decoded/unescaped
/// or kept as raw bytes is to be settled together with the quick-xml version
/// selection (see Open Question 3).
pub(crate) fn required_attr(
    start: &BytesStart<'_>,
    path: &str,
    name: &'static str,
) -> Result<String, Error> {
    let _ = (start, path, name);
    unimplemented!()
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
pub(crate) fn concat_rich_text<R: BufRead>(
    reader: &mut Reader<R>,
    buf: &mut Vec<u8>,
) -> Result<String, Error> {
    let _ = (reader, buf);
    unimplemented!()
}
```

## Dependencies

- Depends on: [`container/sanitize.rs`](../container/sanitize.en.md) (only for downcasting to `LimitExceeded` — no dependency on `container::ZipContainer` itself; architecture.md design policy 3 forbids `container` and `parse` from knowing about each other's orchestration role directly, and referencing this single internal error type does not violate that), [`error.rs`](../error.en.md), and the external `quick-xml` crate
- Depended on by: every submodule under `parse/` ([relationships.rs](relationships.en.md) / [workbook.rs](workbook.en.md) / [shared_strings.rs](shared_strings.en.md) / [styles.rs](styles.en.md) / [worksheet.rs](worksheet.en.md)), `pipeline.rs` (calls the re-exported parse functions)

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
- **`read_event`: verify that feeding a malicious XML payload containing a DOCTYPE declaration and an external entity reference (e.g. `<!DOCTYPE foo [ <!ENTITY xxe SYSTEM "file:///etc/passwd"> ]>`) returns `Error::DoctypeRejected` the moment `Event::DocType` is detected, and reads no further events** (verifies requirements chapter 2's XXE requirement itself; a regression test for the explicit, verifiable mitigation [the security review](../../security/design-review.en.md) Finding 1 called for, instead of resting on an implicit assumption alone)
- `read_event`: verify that legitimate XML with no external entity reference and no DOCTYPE declaration (representative of `_rels`/`workbook.xml`/`sharedStrings.xml`/`styles.xml`/`sheetX.xml`) returns events as normal, and `Error::DoctypeRejected` is never raised spuriously (a regression test guarding against false positives on a legitimate `.xlsx`)
- `read_event`: verify that XML with no DOCTYPE but a syntax error, and input that exceeds `BoundedReader`'s limit, each convert correctly to `Error::XmlParse`/`Error::ZipBombDetected` via `convert_xml_error` (a wiring test confirming error conversion happens before the `Event::DocType` check)
- None of the above XXE-related tests are repeated in individual `parse/*.rs` files (per the [PR #9 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204), they are centralized here, where `read_event` is defined)

## Open Questions

1. **Finalizing the quick-xml version and `Reader` configuration API**: tied to [error.md Open Question 1](../error.en.md) and [container/mod.md Open Question 1](../container/mod.en.md). `Reader::config_mut()`'s availability and the names of its settings vary by version, so this file's code sample will need updating to match the actual API once `Cargo.toml` is set up. **`read_event`'s XXE mitigation (unconditionally rejecting `Event::DocType`), however, does not depend on how this open question resolves** (reflects [the security review](../../security/design-review.en.md) Finding 1). The `Event` enum's shape (the existence of the `DocType` variant) has stayed stable across quick-xml's major versions, so changes to configuration-API details like `Reader::config_mut()` have no bearing on this mitigation's effectiveness. That independence is exactly what fundamentally resolves the issue Finding 1 raised with the original design, which rested on "the `Reader`'s default behavior" as an implicit assumption.
2. ~~Where the XXE-non-applicability test lives~~ → **Resolved**: centralized in this file's unit tests (reflects the [PR #9 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204)). Placing it alongside where `read_event` is defined lets it directly verify the core of the mitigation — that `Event::DocType` is reliably detected and rejected — without repeating it in individual `parse/*.rs` files.
3. **`required_attr`'s return type**: returning `Cow<str>` or `&str` instead of an allocated `String` could avoid unnecessary allocation, but this is to be settled together with quick-xml's attribute-decoding API (version-dependent).
4. ~~How to resolve namespaces (e.g. `r:id`)~~ → **Resolved**: does not adopt `quick_xml::NsReader`'s namespace-URI-based resolution; simplifies to plain string-prefix matching (matching against an attribute name that includes the prefix, e.g. `"r:id"`, when calling `required_attr`) — reflects the [PR #9 review](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204). This prioritizes requirements chapter 1's "lightweight and fast" policy, given that every major producer (Excel, Google Sheets, LibreOffice, Apache POI, etc.) uses `r` as the relationships-namespace prefix without exception in practice. Even in the very rare case of a legally-formed document that declares the namespace under a different alias, the attribute simply comes back "not found," failing closed as `Error::MissingRequiredElement` rather than silently reading a wrong value.
5. **`Reader` internal buffer sizing for large streams such as `worksheet.xml`**: quick-xml grows its buffer dynamically by default, but there is room to explicitly tune the initial buffer size for the "grid-paper Excel" sheet sizes the requirements target. To be settled during [worksheet.md](worksheet.en.md)'s design/implementation based on profiling.
