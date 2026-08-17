# `parse/mod.rs` Design Doc

*[日本語](mod.md)*

Design doc for `src/parse/mod.rs`. Per [architecture.md](../architecture.en.md), `parse/` is "the layer that consolidates dependence on XML-parsing libraries such as `quick-xml`." This file gathers the cross-cutting concerns shared by the individual parsers ([relationships.md](relationships.en.md) / [workbook.md](workbook.en.md) / [shared_strings.md](shared_strings.en.md) / [styles.md](styles.en.md) / [worksheet.md](worksheet.en.md)): secure `Reader` construction, XML parse-error conversion, and shared helpers for attribute lookup and rich-text concatenation. The conversion logic that [container/sanitize.md Error Handling Policy](../container/sanitize.en.md) said would be "co-located with `parse/mod.rs`'s secure Reader factory" (`convert_xml_error`) is finalized here.

## Responsibility / Scope

- Declares submodules (`mod relationships; mod workbook; mod shared_strings; mod styles; mod worksheet;`) and re-exports crate-internal types
- Provides `create_secure_reader`, the sole gateway for constructing a `quick_xml::Reader` with XXE mitigations already applied. Every module under `parse/` must obtain its `Reader` only through this function, never by calling `Reader::from_reader` directly (this implements architecture.md's rationale: "if each parser initializes its own `Reader`, there is a risk of a missed configuration")
- Provides `convert_xml_error`, the sole gateway for converting `quick_xml::Error` into [`crate::error::Error`](../error.en.md). This is also where limit-exceeded errors from [container/sanitize.md](../container/sanitize.en.md)'s `BoundedReader` (Zip Bomb protection) are detected and converted into `Error::ZipBombDetected`
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
pub(crate) use worksheet::{WorksheetParseOutput, parse_worksheet};

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

## Error Handling Policy

- `create_secure_reader` cannot fail (constructing a `Reader` never fails by itself; I/O errors on the underlying stream only surface when `read_event` is actually called)
- `convert_xml_error` always converts any `quick_xml::Error` into some variant of `crate::error::Error` (never panics). Anything that doesn't match `Error::ZipBombDetected` always falls back to `Error::XmlParse`, so no unknown variant is silently swallowed
- `required_attr` returns `Result` rather than panicking, since a missing attribute can originate from untrusted external input (a malformed `.xlsx`)

## Testing Strategy

- Verify `create_secure_reader` produces a `Reader` with the expected configuration (`trim_text(false)`, etc.)
- `convert_xml_error`: verify that a `quick_xml::Error::Io` wrapping the `io::Error` produced by `BoundedReader`'s `LimitExceeded` converts correctly to `Error::ZipBombDetected`, preserving `limit`/`actual`
- `convert_xml_error`: verify that an ordinary XML syntax error (e.g. an unclosed tag) converts to `Error::XmlParse` with `path` set correctly
- `required_attr`: verify it retrieves the value when the attribute is present, and returns `Error::MissingRequiredElement` when absent
- `concat_rich_text`: verify a single `<t>`, multiple `<r><t>` runs, and input containing `<rPh>` each produce the expected string (exhaustive cases live in [shared_strings.md Testing Strategy](shared_strings.en.md); this file only verifies the wiring)
- An integration-style test that feeds a malicious XML payload (a DOCTYPE declaration with an external entity reference — an XXE payload) through `create_secure_reader` and verifies that no content from the external file ever appears in the parse result (verifies requirements chapter 2's XXE requirement itself; whether this is centralized here or also repeated in individual `parse/*.rs` files is undecided — see Open Question 2)

## Open Questions

1. **Finalizing the quick-xml version and `Reader` configuration API**: tied to [error.md Open Question 1](../error.en.md) and [container/mod.md Open Question 1](../container/mod.en.md). `Reader::config_mut()`'s availability and the names of its settings vary by version, so this file's code sample will need updating to match the actual API once `Cargo.toml` is set up.
2. **Where the XXE-non-applicability test lives**: undecided whether to centralize it here or also repeat it in individual `parse/*.rs` files (especially [relationships.rs](relationships.en.md), which is first to receive external input).
3. **`required_attr`'s return type**: returning `Cow<str>` or `&str` instead of an allocated `String` could avoid unnecessary allocation, but this is to be settled together with quick-xml's attribute-decoding API (version-dependent).
4. **How to resolve namespaces (e.g. `r:id`)**: attribute matching that depends on OOXML's `r` namespace prefix — `workbook.xml`'s `<sheet r:id="...">`, certain attributes on worksheet cells — appears in several modules including [workbook.md](workbook.en.md). The prefix `r` is conventionally near-universal, but technically a document could legally declare it under a different alias (e.g. `xmlns:foo="http://schemas.openxmlformats.org/officeDocument/2006/relationships"`), so it's undecided whether `create_secure_reader` should return `quick_xml::NsReader` (namespace-URI-based resolution) instead of plain string-prefix matching. Adopting it affects the API across all of `parse/`, so the decision must be made here, once, for the whole layer.
5. **`Reader` internal buffer sizing for large streams such as `worksheet.xml`**: quick-xml grows its buffer dynamically by default, but there is room to explicitly tune the initial buffer size for the "grid-paper Excel" sheet sizes the requirements target. To be settled during [worksheet.md](worksheet.en.md)'s design/implementation based on profiling.
