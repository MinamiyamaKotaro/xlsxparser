// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Phase 1: parses a `_rels/*.rels` part (e.g.
//! `xl/_rels/workbook.xml.rels`) into a routing map from `r:id` to the
//! target part.

use crate::error::Error;
use crate::parse::{create_secure_reader, optional_attr, read_event, required_attr};
use quick_xml::events::Event;
use std::collections::HashMap;
use std::io::BufRead;

/// A single `<Relationship>` element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Relationship {
    /// r:id (e.g. "rId1").
    pub id: String,
    /// The full URI from the Type attribute (e.g. ".../relationships/worksheet").
    /// Kept as a plain string; interpreting it is left to the caller.
    pub rel_type: String,
    /// For Internal: the ZIP-entry-name-equivalent absolute path already
    /// resolved by `resolve_target_path`. For External: the Target
    /// attribute's URI string, unchanged.
    pub target: String,
    pub target_mode: TargetMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetMode {
    /// The default (when the TargetMode attribute is absent). Refers to a
    /// part inside the ZIP archive.
    Internal,
    /// Refers to an external URI (http(s), etc.). This library never
    /// fetches it.
    External,
}

/// Routing map from r:id to `Relationship`.
pub(crate) type RelationshipMap = HashMap<String, Relationship>;

/// Parses a `_rels` part's XML (e.g. the contents of
/// `xl/_rels/workbook.xml.rels`) and builds a `RelationshipMap`.
///
/// `part_dir` is the directory of the part this rels part is associated
/// with (e.g. `"xl"` for the rels belonging to `xl/workbook.xml`) — the
/// anchor used to resolve `Target`'s relative paths. `path` is an
/// identifier used only in error messages (the rels part's own ZIP entry
/// name, e.g. `"xl/_rels/workbook.xml.rels"`).
pub(crate) fn parse_relationships(
    reader: impl BufRead,
    part_dir: &str,
    path: &str,
) -> Result<RelationshipMap, Error> {
    let mut xml_reader = create_secure_reader(reader);
    let mut buf = Vec::new();
    let mut map = RelationshipMap::new();

    loop {
        let event = read_event(&mut xml_reader, &mut buf, path)?;
        match &event {
            Event::Start(start) | Event::Empty(start)
                if start.local_name().as_ref() == b"Relationship" =>
            {
                let id = required_attr(start, path, "Id")?;
                let rel_type = required_attr(start, path, "Type")?;
                let target_attr = required_attr(start, path, "Target")?;
                let target_mode = match optional_attr(start, path, "TargetMode")?.as_deref() {
                    Some("External") => TargetMode::External,
                    _ => TargetMode::Internal,
                };
                let target = match target_mode {
                    TargetMode::External => target_attr,
                    TargetMode::Internal => resolve_target_path(part_dir, &target_attr),
                };
                map.insert(
                    id.clone(),
                    Relationship {
                        id,
                        rel_type,
                        target,
                        target_mode,
                    },
                );
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(map)
}

/// Resolves the relative-path notation `target` from a rels part, anchored
/// at `base_dir` (the directory of the part the rels belongs to), into a
/// ZIP-entry-name-equivalent absolute path. Since OPC part names are always
/// `/`-delimited, this processes the string segment-by-segment manually
/// rather than using `std::path::Path`, avoiding OS-dependent path
/// interpretation (e.g. Windows' `\` separator).
///
/// Per OPC (ECMA-376 Part 2), a `target` starting with `/` is
/// package-absolute and resolved from the package root, ignoring
/// `base_dir` entirely, rather than being appended to it — tools such as
/// openpyxl emit worksheet relationships this way (e.g.
/// `Target="/xl/worksheets/sheet1.xml"`), so without this case every sheet
/// in such a package would fail to resolve as a dangling relationship.
///
/// A `..` segment is handled naively by popping the previous segment; this
/// function alone does not guarantee well-defined behavior for a `..` that
/// goes deeper than `base_dir` (e.g. `base_dir` = `"xl"`, `target` =
/// `"../../evil"`) — final safety is left to
/// `container::ZipContainer::get_entry`'s independent re-validation
/// (`validate_entry_path`) of whatever path this function produces (defense
/// in depth).
fn resolve_target_path(base_dir: &str, target: &str) -> String {
    let mut segments: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        base_dir.split('/').filter(|s| !s.is_empty()).collect()
    };
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            seg => segments.push(seg),
        }
    }
    segments.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &[u8], part_dir: &str) -> RelationshipMap {
        parse_relationships(xml, part_dir, "xl/_rels/workbook.xml.rels").unwrap()
    }

    #[test]
    fn parses_multiple_relationships() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>
</Relationships>"#;

        let map = parse(xml, "xl");
        assert_eq!(map.len(), 2);
        assert_eq!(map["rId1"].target, "xl/worksheets/sheet1.xml");
        assert_eq!(map["rId1"].target_mode, TargetMode::Internal);
        assert_eq!(map["rId2"].target, "xl/sharedStrings.xml");
    }

    #[test]
    fn missing_id_is_an_error() {
        let xml = br#"<Relationships><Relationship Type="t" Target="a"/></Relationships>"#;
        let err = parse_relationships(&xml[..], "xl", "path").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement { name: "Id", .. }
        ));
    }

    #[test]
    fn missing_type_is_an_error() {
        let xml = br#"<Relationships><Relationship Id="rId1" Target="a"/></Relationships>"#;
        let err = parse_relationships(&xml[..], "xl", "path").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement { name: "Type", .. }
        ));
    }

    #[test]
    fn missing_target_is_an_error() {
        let xml = br#"<Relationships><Relationship Id="rId1" Type="t"/></Relationships>"#;
        let err = parse_relationships(&xml[..], "xl", "path").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement { name: "Target", .. }
        ));
    }

    #[test]
    fn external_target_mode_keeps_target_verbatim() {
        let xml = br#"<Relationships><Relationship Id="rId3" Type="t" Target="https://example.com/x" TargetMode="External"/></Relationships>"#;
        let map = parse(xml, "xl");
        assert_eq!(map["rId3"].target, "https://example.com/x");
        assert_eq!(map["rId3"].target_mode, TargetMode::External);
    }

    #[test]
    fn empty_relationships_produces_empty_map() {
        let xml = b"<Relationships></Relationships>";
        assert!(parse(xml, "xl").is_empty());
    }

    #[test]
    fn resolve_target_path_simple_relative() {
        assert_eq!(
            resolve_target_path("xl", "worksheets/sheet1.xml"),
            "xl/worksheets/sheet1.xml"
        );
    }

    #[test]
    fn resolve_target_path_with_parent_dir() {
        assert_eq!(
            resolve_target_path("xl/worksheets", "../media/image1.png"),
            "xl/media/image1.png"
        );
    }

    #[test]
    fn resolve_target_path_excessive_parent_dir_does_not_panic() {
        // Popping past an empty `segments` must not panic; the result is
        // whatever it is — final rejection is `container::get_entry`'s job.
        let result = resolve_target_path("xl", "../../evil");
        assert_eq!(result, "evil");
    }

    #[test]
    fn resolve_target_path_package_absolute_target_ignores_base_dir() {
        // openpyxl-style: Target="/xl/worksheets/sheet1.xml" from a rels
        // part anchored at "xl". Must resolve from the package root, not
        // "xl" + "/xl/worksheets/sheet1.xml" = "xl/xl/worksheets/sheet1.xml".
        assert_eq!(
            resolve_target_path("xl", "/xl/worksheets/sheet1.xml"),
            "xl/worksheets/sheet1.xml"
        );
    }

    #[test]
    fn resolve_target_path_package_absolute_target_with_excessive_parent_dir_does_not_panic() {
        let result = resolve_target_path("xl", "/../../evil");
        assert_eq!(result, "evil");
    }
}
