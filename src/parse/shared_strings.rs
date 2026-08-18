// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Parses `xl/sharedStrings.xml` into a `SharedStringTable` — the SST index
//! order (source `<si>` order) is preserved, since `t="s"` cells reference
//! entries by that index.

use crate::error::Error;
use crate::parse::{concat_rich_text, create_secure_reader, read_event};
use quick_xml::events::Event;
use std::io::BufRead;
use std::sync::Arc;

/// The shared string table. `resolve/shared_strings.rs` uses it to resolve
/// `t="s"` cells' indices.
#[derive(Debug, Clone, Default)]
pub(crate) struct SharedStringTable {
    strings: Vec<Arc<str>>,
}

impl SharedStringTable {
    /// Looks up the string at an index. Used by
    /// `resolve/shared_strings.rs::resolve` to build
    /// `Error::SharedStringIndexOutOfBounds` when out of range.
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
pub(crate) fn parse_shared_strings(
    reader: impl BufRead,
    path: &str,
) -> Result<SharedStringTable, Error> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &[u8]) -> SharedStringTable {
        parse_shared_strings(xml, "xl/sharedStrings.xml").unwrap()
    }

    #[test]
    fn plain_strings_preserve_index_order() {
        let xml = br#"<sst><si><t>hello</t></si><si><t>world</t></si></sst>"#;
        let table = parse(xml);
        assert_eq!(table.len(), 2);
        assert_eq!(table.get(0).unwrap().as_ref(), "hello");
        assert_eq!(table.get(1).unwrap().as_ref(), "world");
    }

    #[test]
    fn rich_text_runs_concatenate() {
        // xml:space="preserve" marks the meaningful trailing space (Issue
        // #56) — without it, `parse::concat_rich_text` now correctly trims
        // each run's own leading/trailing whitespace.
        let xml =
            br#"<sst><si><r><t xml:space="preserve">hello </t></r><r><t>world</t></r></si></sst>"#;
        let table = parse(xml);
        assert_eq!(table.get(0).unwrap().as_ref(), "hello world");
    }

    #[test]
    fn rph_furigana_is_excluded() {
        let xml =
            br#"<sst><si><t>&#x5C71;&#x7530;</t><rPh sb="0" eb="2"><t>&#x3084;&#x307E;&#x3060;</t></rPh></si></sst>"#;
        let table = parse(xml);
        assert_eq!(table.get(0).unwrap().as_ref(), "\u{5C71}\u{7530}");
    }

    #[test]
    fn named_entities_are_resolved() {
        let xml = br#"<sst><si><t>Tom &amp; Jerry &lt;3&gt;</t></si></sst>"#;
        let table = parse(xml);
        assert_eq!(table.get(0).unwrap().as_ref(), "Tom & Jerry <3>");
    }

    #[test]
    fn preserve_whitespace_is_kept() {
        let xml = br#"<sst><si><t xml:space="preserve">  padded  </t></si></sst>"#;
        let table = parse(xml);
        assert_eq!(table.get(0).unwrap().as_ref(), "  padded  ");
    }

    #[test]
    fn empty_si_is_empty_string() {
        let xml = b"<sst><si/></sst>";
        let table = parse(xml);
        assert_eq!(table.len(), 1);
        assert_eq!(table.get(0).unwrap().as_ref(), "");
    }

    #[test]
    fn empty_sst_yields_empty_table() {
        let xml = br#"<sst count="0" uniqueCount="0"></sst>"#;
        let table = parse(xml);
        assert_eq!(table.len(), 0);
    }

    #[test]
    fn get_out_of_range_returns_none() {
        let xml = b"<sst><si><t>only</t></si></sst>";
        let table = parse(xml);
        assert!(table.get(5).is_none());
    }
}
