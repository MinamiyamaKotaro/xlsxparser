//! Shared low-level helpers for constructing minimal, deterministic
//! `.xlsx` (OOXML/ZIP) packages in-memory, without depending on Excel or
//! any external xlsx-writing library. Each sibling module in `fixtures/`
//! (`normal`, `error`, `complex`, `load`, `security`) builds one category's
//! scenarios on top of these.
//!
//! `pipeline.rs`'s own `#[cfg(test)]` module already established this
//! pattern (a `build_zip` helper plus hand-written OOXML part strings) for
//! its unit tests; this module generalizes it so integration tests can
//! reuse the same approach instead of committing opaque binary `.xlsx`
//! fixtures (which can't be code-reviewed as a diff, and for the
//! `security` category specifically would mean carrying live attack
//! payloads as repository bytes).

use std::io::{Cursor, Write};

/// The package root's `officeDocument` relationship (Issue #55), pointing
/// at the conventional `xl/workbook.xml` every fixture in this module tree
/// uses. `build_zip` injects this automatically so none of the ~90 existing
/// call sites across `fixtures/*.rs` had to change when Issue #55's
/// pipeline started reading `_rels/.rels` before anything else.
const DEFAULT_ROOT_RELS_XML: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#;

const ROOT_RELS_PATH: &str = "_rels/.rels";

/// Packs `entries` (zip entry name -> raw bytes) into an in-memory ZIP
/// archive using the `Deflated` compression method, matching what real
/// `.xlsx` writers produce. Injects [`DEFAULT_ROOT_RELS_XML`] at
/// [`ROOT_RELS_PATH`] unless `entries` already supplies that path itself
/// (for fixtures that specifically need to test root-rels handling).
pub fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        if !entries.iter().any(|(name, _)| *name == ROOT_RELS_PATH) {
            writer.start_file(ROOT_RELS_PATH, options).unwrap();
            writer.write_all(DEFAULT_ROOT_RELS_XML).unwrap();
        }
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }
    buf
}

/// Same as [`build_zip`], but for entries whose name and/or content is
/// produced dynamically (`String`/`Vec<u8>`) rather than borrowed from a
/// `&'static` slice — used by the `load` fixtures (e.g. `thousand_sheets`,
/// which generates 1000 distinct entry names at runtime).
pub fn build_zip_owned(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let borrowed: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(name, data)| (name.as_str(), data.as_slice()))
        .collect();
    build_zip(&borrowed)
}

/// Builds `xl/_rels/workbook.xml.rels` from `(id, relationship_type_suffix,
/// target)` triples, e.g. `("rId1", "worksheet",
/// "worksheets/sheet1.xml")`. `relationship_type_suffix` is appended to the
/// standard `.../officeDocument/2006/relationships/` prefix.
pub fn rels_xml(relationships: &[(&str, &str, &str)]) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n",
    );
    for (id, type_suffix, target) in relationships {
        xml.push_str(&format!(
            "  <Relationship Id=\"{id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/{type_suffix}\" Target=\"{target}\"/>\n"
        ));
    }
    xml.push_str("</Relationships>");
    xml
}

/// Builds `xl/workbook.xml` from `(name, r_id, state)` triples. `state` is
/// `None` for a visible sheet, or `Some("hidden"/"veryHidden")` — mirrors
/// the OOXML `<sheet state="...">` attribute.
pub fn workbook_xml(sheets: &[(&str, &str, Option<&str>)]) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\"?>\n\
         <workbook xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\n  <sheets>\n",
    );
    for (name, r_id, state) in sheets {
        let state_attr = match state {
            Some(s) => format!(" state=\"{s}\""),
            None => String::new(),
        };
        xml.push_str(&format!(
            "    <sheet name=\"{name}\" sheetId=\"1\" r:id=\"{r_id}\"{state_attr}/>\n"
        ));
    }
    xml.push_str("  </sheets>\n</workbook>");
    xml
}

/// Wraps `rows_xml` (the `<row>` elements) and an optional
/// `<mergeCells>...</mergeCells>` block into a full
/// `xl/worksheets/sheetN.xml` document. Pass `""` for `merge_cells_xml`
/// when the sheet has no merged regions.
pub fn worksheet_xml(rows_xml: &str, merge_cells_xml: &str) -> String {
    format!("<worksheet><sheetData>\n{rows_xml}\n</sheetData>{merge_cells_xml}</worksheet>")
}

/// A minimal `xl/styles.xml` with a single default (non-date) `cellXfs`
/// entry at style id 0.
pub const DEFAULT_STYLES_XML: &[u8] =
    br#"<styleSheet><cellXfs><xf numFmtId="0"/></cellXfs></styleSheet>"#;

/// A `xl/styles.xml` with two `cellXfs` entries: style id 0 = default
/// (General, not a date), style id 1 = a built-in date/time format
/// (`numFmtId="14"`, `"mm-dd-yy"` per ECMA-376).
pub const DATE_STYLES_XML: &[u8] =
    br#"<styleSheet><cellXfs><xf numFmtId="0"/><xf numFmtId="14"/></cellXfs></styleSheet>"#;

/// A `xl/styles.xml` with two `<fonts>` entries (style id 0 = default
/// 11pt/not-bold, style id 1 = 14pt/bold) referenced by two `cellXfs`
/// entries via `fontId`.
pub const FONT_STYLES_XML: &[u8] = br#"<styleSheet>
<fonts count="2">
<font><sz val="11"/><name val="Calibri"/></font>
<font><b/><sz val="14"/><name val="Calibri"/></font>
</fonts>
<cellXfs><xf fontId="0"/><xf fontId="1"/></cellXfs>
</styleSheet>"#;

/// A minimal `xl/sharedStrings.xml` built from `strings`, preserving index
/// order (SST-referencing cells use `t="s"` with this order as their
/// index).
pub fn shared_strings_xml(strings: &[&str]) -> String {
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" count=\"{0}\" uniqueCount=\"{0}\">\n",
        strings.len()
    );
    for s in strings {
        xml.push_str(&format!("  <si><t>{s}</t></si>\n"));
    }
    xml.push_str("</sst>");
    xml
}
