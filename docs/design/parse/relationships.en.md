# `parse/relationships.rs` Design Doc

*[日本語](relationships.md)*

Design doc for `src/parse/relationships.rs`. Per [architecture.md](../architecture.en.md), this implements Phase 1: "relationship (`_rels`) parsing — parsing the data used to build the routing map." It parses the XML structure common to every OPC (Open Packaging Conventions) `_rels/*.rels` part (`<Relationships><Relationship .../></Relationships>`), such as `xl/_rels/workbook.xml.rels`. This file also owns resolving the target path — something [container/mod.md Dependencies](../container/mod.en.md) already assumed when it said "`parse/relationships.rs` (Phase 1) dynamically computes [a path] by combining a relative-path notation from a `.rels` file with an entry name."

## Responsibility / Scope

- Parses a `_rels/*.rels` part's XML and builds a `RelationshipMap` keyed by `r:id` (Relationship ID)
- Resolves each `<Relationship>`'s `Target` attribute (a path relative to the rels part itself — e.g. `worksheets/sheet1.xml`, `../media/image1.png`) into a ZIP-entry-name-equivalent absolute path (e.g. `xl/worksheets/sheet1.xml`), anchored at the directory the rels part belongs to
- Distinguishes `TargetMode="External"` (a relationship pointing at an external URI) from internal parts. Since this library never fetches resources outside the archive, an external relationship's target is kept as-is (the URI string) without path resolution
- **Not responsible for**: assigning meaning to which `r:id` corresponds to which OOXML part kind (worksheet/sharedStrings/styles, etc.) or filtering by it (the caller, `pipeline.rs` — this file parses any `_rels` part generically and never interprets `Relationship.rel_type`'s string), verifying that a resolved target path actually exists in the ZIP archive (represented as `Ok(None)` by [`container::ZipContainer::get_entry`](../container/mod.en.md))

## Key Types / Functions (draft)

```rust
use crate::error::Error;
use crate::parse::{convert_xml_error, create_secure_reader, required_attr};
use std::collections::HashMap;
use std::io::BufRead;

/// A single `<Relationship>` element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Relationship {
    /// r:id (e.g. "rId1").
    pub id: String,
    /// The full URI from the Type attribute (e.g. ".../relationships/worksheet").
    /// Kept as a plain string; interpreting it is left to the caller (see
    /// Open Question 3).
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
    let _ = (&mut xml_reader, part_dir, path);
    // Implementation plan: for each <Relationship> element, read Id/Type/
    // Target/TargetMode via required_attr etc., and only resolve `target`
    // via resolve_target_path when TargetMode is Internal (the default).
    unimplemented!()
}

/// Resolves the relative-path notation `target` from a rels part, anchored
/// at `base_dir` (the directory of the part the rels belongs to), into a
/// ZIP-entry-name-equivalent absolute path. Since OPC part names are always
/// `/`-delimited, this processes the string segment-by-segment manually
/// rather than using `std::path::Path`, avoiding OS-dependent path
/// interpretation (e.g. Windows' `\` separator).
///
/// A `..` segment is handled naively by popping the previous segment
/// (parent-directory reference); this function alone does not guarantee
/// well-defined behavior for a `..` that goes deeper than `base_dir`
/// (e.g. `base_dir` = `"xl"`, `target` = `"../../evil"`) — see Dependencies.
fn resolve_target_path(base_dir: &str, target: &str) -> String {
    let mut segments: Vec<&str> = base_dir.split('/').filter(|s| !s.is_empty()).collect();
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
```

## Dependencies

- Depends on: [`parse/mod.rs`](mod.en.md) (`create_secure_reader`, `convert_xml_error`, `required_attr`), [`error.rs`](../error.en.md)
- Depended on by: `pipeline.rs` (parses `xl/_rels/workbook.xml.rels` during Phase 1 to build the routing map for sheet IDs, shared strings, and styles. Per architecture.md — "once the routing map is built, the `_rels` scratch buffer is discarded immediately at the end of Phase 1" — the `RelationshipMap` itself is expected to be dropped once Phase 1 completes)

If `resolve_target_path` receives a `..` that goes deeper than `base_dir`, `Vec::pop()` on an already-empty vector simply does nothing (returns `None`), so the function can produce an unintentionally shallow path (in the worst case, an empty string). This is exactly the premise [container/mod.md Dependencies](../container/mod.en.md) already spelled out: "the `name` passed to `get_entry` may be a value `parse/relationships.rs` computed dynamically, and `get_entry` re-validates independently in case that computation had a normalization gap" — defense in depth. So this function deliberately does not reject such malformed input up front; final safety is left to the re-validation [`container::ZipContainer::get_entry`](../container/mod.en.md) performs (`validate_entry_path`) on every call (see Open Question 2).

## Error Handling Policy

- Returns `Error::MissingRequiredElement` if a `<Relationship>`'s `Id`, `Type`, or `Target` attribute is absent (`TargetMode` is optional and defaults to `Internal`)
- Converts structurally invalid XML into `Error::XmlParse` or `Error::ZipBombDetected` via [`convert_xml_error`](mod.en.md)
- `resolve_target_path` itself never returns an error (never panics; it always returns some string). Final defense against a malformed path is delegated to the caller (`container::get_entry`) — see Dependencies

## Testing Strategy

- Verify that a valid `_rels` XML with multiple `<Relationship>` elements produces the expected `RelationshipMap` (contents keyed by `id`)
- Verify that a `<Relationship>` missing `Id`, `Type`, or `Target` returns `Error::MissingRequiredElement`
- `resolve_target_path`: verify a simple relative path (`"worksheets/sheet1.xml"`) is correctly joined with `base_dir` into an absolute path
- `resolve_target_path`: verify a relative path containing a parent-directory reference (`base_dir = "xl/worksheets"`, `target = "../media/image1.png"` → `"xl/media/image1.png"`) resolves correctly
- `resolve_target_path`: verify that passing a `..` deeper than `base_dir` (e.g. `base_dir = "xl"`, `target = "../../evil"`) does not panic and still returns some string, and that the path from that route is ultimately rejected as `Error::ZipSlipDetected` by `container::get_entry`'s re-validation (a regression test wiring this to [container/mod.md](../container/mod.en.md))
- Verify that a `<Relationship>` with `TargetMode="External"` keeps its `Target` string verbatim in `target`, bypassing `resolve_target_path`
- Verify that an empty `<Relationships>` with no children produces an empty `RelationshipMap`

## Open Questions

1. ~~Scope of `_rels` parts this file is meant to parse~~ → **Resolved**: originally designed as a generic `_rels` parser not limited to any one part, with media-embedding support left undecided. Issue [#65](https://github.com/MinamiyamaKotaro/xlsxparser/issues/65) answered this: the existing generic parser is reused as-is for `xl/worksheets/_rels/sheetN.xml.rels` and `xl/drawings/_rels/drawingN.xml.rels` too, with no changes to this file — see [drawing.md](drawing.en.md). `[Content_Types].xml` rels support remains undecided.
2. **Handling an excessive `..` in `resolve_target_path`**: currently the function itself never errors, relying entirely on `container::get_entry`'s re-validation as the final defense (defense in depth). Whether cases this function can already detect unambiguously — `segments` ending up empty, or escaping outside `base_dir` — should be rejected early here as `Error::ZipSlipDetected`-equivalent is worth revisiting as a question of how responsibility is split across the defense-in-depth layers.
3. **`Relationship.rel_type`'s type**: currently kept as a full URI string (`String`), meaning the caller (`pipeline.rs`) does a string comparison every time it needs to identify a known relationship type (worksheet/sharedStrings/styles, etc.). Whether to predefine an `enum` for known types and convert into it is to be settled during `pipeline.rs`'s design.
4. ~~Namespace handling~~ → **Resolved**: follows the policy [parse/mod.md Open Question 4](mod.en.md) settled on — plain string-prefix matching, no `quick_xml::NsReader`. The `_rels` XML itself has a fixed namespace (`http://schemas.openxmlformats.org/package/2006/relationships`), but none of its element/attribute names (`Relationship`, `Id`, `Type`, `Target`, `TargetMode`) carry a prefix, so this file is affected less than other modules.
