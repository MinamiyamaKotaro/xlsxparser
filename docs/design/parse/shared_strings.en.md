# `parse/shared_strings.rs` Design Doc

*[日本語](shared_strings.md)*

Design doc for `src/parse/shared_strings.rs`. Per [architecture.md](../architecture.en.md), this implements the `parse/` responsibility "parsing `sharedStrings.xml` (extracting the SST's structured data)." It parses `xl/sharedStrings.xml` and builds the `SharedStringTable` consumed by [resolve/shared_strings.md](../resolve/shared_strings.en.md). It also resolves that file's [Open Question 1](../resolve/shared_strings.en.md) ("`SharedStringTable`'s type and location are to be settled when `parse/shared_strings.rs` is designed").

## Responsibility / Scope

- Parses `xl/sharedStrings.xml`'s `<sst><si>...</si>...</sst>` and builds a `SharedStringTable` that holds strings in `<si>`'s source order (i.e. shared-string index order)
- For each `<si>`, resolves to a single `String` containing only the concatenated text — whether it's a plain `<t>...</t>` or rich-text runs (`<r><t>...</t></r>...`) — discarding formatting info (`<rPr>`), matching how [model/cell.md](../model/cell.en.md)'s `CellValue::Text` holds only a plain string
- Avoids accidentally concatenating `<t>` nested under `<rPh>` (phonetic annotations — furigana Excel sometimes auto-generates for Japanese names, place names, etc.) into the main text
- Preserves leading/trailing whitespace for a `<t>` carrying `xml:space="preserve"` (implements requirements chapter 4's "must honor `xml:space=\"preserve\"`")
- **Not responsible for**: resolving a `t="s"` cell's index into the actual string ([`resolve/shared_strings.rs`](../resolve/shared_strings.en.md) — this file only builds the table that resolution uses), resolving formula-result strings (`t="str"`) or inline strings (`t="inlineStr"`) (handled directly during streaming by [`parse/worksheet.rs`](worksheet.en.md); see [resolve/shared_strings.md](../resolve/shared_strings.en.md))

## Key Types / Functions (draft)

```rust
use crate::error::Error;
use crate::parse::{concat_rich_text, create_secure_reader, read_event};
use quick_xml::events::Event;
use std::io::BufRead;
use std::sync::Arc;

/// The shared string table. [resolve/shared_strings.rs](../resolve/shared_strings.en.md)
/// uses it to resolve `t="s"` cells' indices (this resolves that file's
/// Open Question 1).
#[derive(Debug, Clone, Default)]
pub(crate) struct SharedStringTable {
    strings: Vec<Arc<str>>,
}

impl SharedStringTable {
    /// Looks up the string at an index. Used by `resolve/shared_strings.rs::resolve`
    /// to build `Error::SharedStringIndexOutOfBounds` when out of range.
    pub(crate) fn get(&self, index: usize) -> Option<&Arc<str>> {
        self.strings.get(index)
    }

    pub(crate) fn len(&self) -> usize {
        self.strings.len()
    }
}

/// Parses `xl/sharedStrings.xml` and builds a `SharedStringTable`. Each
/// `<si>` under `<sst>` resolves to a single string via `concat_rich_text`
/// (plain `<t>`, rich-text runs, or empty for a bare `<si/>`).
pub(crate) fn parse_shared_strings(reader: impl BufRead, path: &str) -> Result<SharedStringTable, Error> {
    let mut xml_reader = create_secure_reader(reader);
    let mut buf = Vec::new();
    let mut strings = Vec::new();
    loop {
        let event = read_event(&mut xml_reader, &mut buf, path)?;
        match &event {
            Event::Start(e) if e.local_name().as_ref() == b"si" => {
                buf.clear();
                let text = concat_rich_text(&mut xml_reader, path)?;
                strings.push(Arc::from(text));
                continue;
            }
            Event::Empty(e) if e.local_name().as_ref() == b"si" => {
                strings.push(Arc::from(""));
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(SharedStringTable { strings })
}
```

## Dependencies

- Depends on: [`parse/mod.rs`](mod.en.md) (`create_secure_reader`, `read_event`, `concat_rich_text`), [`error.rs`](../error.en.md). No dependency on `model/` — `SharedStringTable` is a `parse/`-internal intermediate data structure; converting to `model::CellValue::Text` is [`resolve/shared_strings.rs`](../resolve/shared_strings.en.md)'s job
- Depended on by: [`resolve/shared_strings.rs`](../resolve/shared_strings.en.md) (`SharedStringTable::get`), [`resolve/mod.rs`](../resolve/mod.en.md) (passed through as an argument to `resolve_sheet`), `pipeline.rs` (built once between Phases 1–3 and passed into every `resolve_sheet` call; per architecture.md — "`SharedStringTable` and `StyleSheet` are discarded once Phase 4 completes" — it is dropped once every sheet has finished resolving)

`resolve/shared_strings.rs` importing `SharedStringTable` directly from `parse::shared_strings` (bypassing `model/`) is exactly the implementation of what [resolve/mod.md Dependencies](../resolve/mod.en.md) had already reasoned through: "the dependency on `parse::shared_strings::SharedStringTable` is not the 'dependency on I/O' that architecture.md design policy 2 forbids — it's a dependency on structured, in-memory data that Phase 3 has already built."

## Error Handling Policy

- Structurally invalid XML is converted into `Error::XmlParse` or `Error::ZipBombDetected` via [`convert_xml_error`](mod.en.md)
- An `<si>`'s structure itself (neither `<t>` nor `<r>`, i.e. an empty `<si/>` with no text) is treated as an empty string rather than an error — Excel legitimately can produce a shared-string entry for an empty cell
- A failure to interpret an individual `<si>` never panics (the input is untrusted external data)

## Testing Strategy

- Verify that multiple plain `<si><t>...</t></si>` entries are stored in `SharedStringTable` in the correct order (source order = index order)
- Verify that an `<si>` with rich text (multiple `<r><t>...</t></r>`) resolves to a single string concatenating each run's text
- Verify that `<t>` nested under `<rPh>` (furigana) is not accidentally concatenated into the main text (a regression test specific to Japanese business use cases, matching the context requirements chapter 1 targets)
- Verify that whitespace in a `<t xml:space="preserve"> ... </t>` with leading/trailing spaces is preserved, not trimmed
- Verify that an empty `<si/>` (no text) is treated as an empty string and does not fail the parse as a whole
- Verify that an empty `<sst>` (`uniqueCount="0"`) yields an empty `SharedStringTable` (`len() == 0`)
- Verify that `SharedStringTable::get` returns `None` for an out-of-range index (the wiring test underlying `resolve/shared_strings.rs`'s error conversion)

## Open Questions

1. ~~Soundness of always preserving whitespace instead of branching on the `xml:space` attribute's value~~ — **Resolved (Issue #56).** [`parse/mod.rs`](mod.en.md)'s `create_secure_reader` still defaults to `trim_text(false)` (quick-xml never sees whitespace-only text nodes as insignificant), but `concat_rich_text` itself now branches per-`<t>` element: it trims each run's leading/trailing whitespace in place unless that specific `<t>` carries `xml:space="preserve"`, matching Excel's own convention. See [parse/mod.md](mod.en.md)'s `concat_rich_text`/`trim_tail_in_place` for the implementation.
2. **Memory-allocation strategy for a large `sharedStrings.xml`** (e.g. a "grid-paper Excel" file with a very large `uniqueCount`): whether to pre-read the `<sst count="N" uniqueCount="M">` element's `uniqueCount` attribute and pre-allocate via `Vec::with_capacity(M)` is to be settled together with performance requirements.
3. **Namespace handling**: same topic as [parse/mod.md Open Question 4](mod.en.md). `<t>` / `<r>` / `<rPh>` themselves carry no prefix, so this file is expected to be affected minimally.
