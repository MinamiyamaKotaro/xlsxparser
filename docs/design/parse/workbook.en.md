# `parse/workbook.rs` Design Doc

*[日本語](workbook.md)*

Design doc for `src/parse/workbook.rs`. Per [architecture.md](../architecture.en.md), this implements the `parse/` responsibility "parsing `workbook.xml`." It converts the `<sheet>` elements under `xl/workbook.xml`'s `<sheets>` into an ordered list that preserves the source's definition order. It directly implements the policy [model/workbook.md Open Question 1](../model/workbook.en.md) settled: "`Workbook.sheets` includes every sheet regardless of visibility, with visibility tracked separately as `SheetVisibility`."

## Responsibility / Scope

- Parses `xl/workbook.xml` and converts each `<sheet name="..." sheetId="..." state="..." r:id="..."/>` under `<sheets>` into a `Vec<WorkbookSheetEntry>` that preserves source order
- Converts the `state` attribute (`"visible"` / `"hidden"` / `"veryHidden"`, defaulting to visible when absent) into [`model::sheet::SheetVisibility`](../model/sheet.en.md)
- Parses `<workbookPr date1904="1"/>` into a `date1904: bool` (Issue #40) — the flag [`resolve/style.rs`](../resolve/style.en.md) needs to pick the correct date-serial epoch when converting `CellValue::Number` to `CellValue::DateTime`. Defaults to `false` (the 1900 system) when `<workbookPr>` or its `date1904` attribute is absent, matching Excel's own default; `"1"`/`"true"` are the only true forms, the same `xsd:boolean` convention [`parse/styles.rs`](styles.en.md) already established for `<b>`/`<alignment wrapText>`
- **Not responsible for**: resolving `r:id` into an actual file path (matching it against the `RelationshipMap` built by [`parse/relationships.rs`](relationships.en.md) is `pipeline.rs`'s job), parsing the sheet's own content (`worksheet.xml` — that's [`parse/worksheet.rs`](worksheet.en.md)), constructing `model::Sheet` itself (`pipeline.rs` builds it, passing `name`/`visibility` from `WorkbookSheetEntry`), the actual serial-to-calendar conversion that consumes `date1904` ([`resolve/style.rs`](../resolve/style.en.md) — this file only carries the flag)

## Key Types / Functions (draft)

```rust
use crate::error::Error;
use crate::model::sheet::SheetVisibility;
use crate::parse::{convert_xml_error, create_secure_reader, required_attr};
use std::io::BufRead;

/// One `<sheet>` entry from `workbook.xml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkbookSheetEntry {
    pub name: String,
    /// The r:id from `<sheets><sheet r:id="rId1" .../></sheets>`. Used by
    /// `pipeline.rs` as the key to look up the actual file path via
    /// `parse::relationships::RelationshipMap`.
    pub r_id: String,
    pub visibility: SheetVisibility,
}

/// The result of parsing `xl/workbook.xml`: the `<sheet>` entries under
/// `<sheets>`, plus the `date1904` flag from `<workbookPr>` (Issue #40).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedWorkbookXml {
    pub sheets: Vec<WorkbookSheetEntry>,
    pub date1904: bool,
}

/// Parses `xl/workbook.xml`. Returns `Error::MissingRequiredElement` if the
/// `<sheets>` element itself is absent. Returns an empty `sheets` Vec if
/// `<sheets></sheets>` is empty (a zero-sheet workbook is structurally
/// valid — see [model/workbook.md Testing Strategy](../model/workbook.en.md)).
pub(crate) fn parse_workbook_xml(reader: impl BufRead, path: &str) -> Result<ParsedWorkbookXml, Error> {
    let mut xml_reader = create_secure_reader(reader);
    let _ = (&mut xml_reader, path);
    unimplemented!()
}

/// Converts the `state` attribute string into `SheetVisibility`. Defaults to
/// `Visible` when the attribute is absent. An unrecognized value (from a
/// future spec extension or a corrupted file) falls back to `Visible`
/// rather than erroring — see Open Question 3.
fn parse_visibility(state: Option<&str>) -> SheetVisibility {
    match state {
        None | Some("visible") => SheetVisibility::Visible,
        Some("hidden") => SheetVisibility::Hidden,
        Some("veryHidden") => SheetVisibility::VeryHidden,
        Some(_) => SheetVisibility::Visible,
    }
}
```

## Dependencies

- Depends on: [`parse/mod.rs`](mod.en.md) (`create_secure_reader`, `convert_xml_error`, `required_attr`), [`model/sheet.rs`](../model/sheet.en.md) (`SheetVisibility`), [`error.rs`](../error.en.md)
- Depended on by: `pipeline.rs` (Phase 1 — matches `r_id` against the `RelationshipMap` built by [`parse/relationships.rs`](relationships.en.md) to determine each sheet's actual file path, constructs `model::Sheet` from `WorkbookSheetEntry`'s `name`/`visibility`, and then hands off Phase 3's streaming parse to [`parse/worksheet.rs`](worksheet.en.md); also carries `ParsedWorkbookXml::date1904` straight through — unstored on `model::Workbook` itself, the same "phase-transient value" treatment `StyleSheet` gets per [architecture.md](../architecture.en.md) — into each sheet's `resolve::resolve_sheet` call)

`date1904` deliberately never becomes a field on the public `model::Workbook` (see [model/workbook.md Open Question](../model/workbook.en.md)): once every sheet's `resolve_sheet` call has consumed it in Phase 4, nothing downstream (JSON output included) needs it again, so keeping it a `pipeline.rs`-local variable avoids growing `Workbook`'s public surface for a value with no further use after resolution.

`parse/workbook.rs` constructs `model::sheet::SheetVisibility` directly. What architecture.md design policy 2 forbids is `resolve/` depending on anything beyond I/O or the model; `model/` itself is meant to be depended on by `parse/` (its own [model/sheet.md Dependencies](../model/sheet.en.md) already lists `parse/worksheet.rs` as a dependent), so a `parse/` → `model/` dependency does not conflict with that policy.

## Error Handling Policy

- Returns `Error::MissingRequiredElement` if the `<sheets>` element itself is absent, since that makes `workbook.xml` structurally invalid
- Returns `Error::MissingRequiredElement` if a `<sheet>`'s `name` or `r:id` attribute is absent
- `date1904` defaults to `false` when `<workbookPr>` or its `date1904` attribute is absent, or when the value is anything other than the `xsd:boolean` true forms (`"1"`/`"true"`) — never an error, since a workbook's date-system flag is not required to make the rest of the document readable
- If `state` is anything other than the three known values (`visible` / `hidden` / `veryHidden`), it falls back to `Visible` rather than being rejected (e.g. as `Error::InvalidPackage`). Visibility is only a display hint and does not affect data integrity, so this applies the same principle [resolve/style.md Error Handling Policy](../resolve/style.en.md) already adopts: "a loose failure interpreting an individual value should not fail the whole document"
- Structurally invalid XML is converted into `Error::XmlParse` or `Error::ZipBombDetected` via [`convert_xml_error`](mod.en.md)

## Testing Strategy

- Verify that a `workbook.xml` with multiple `<sheet>` elements yields a `Vec<WorkbookSheetEntry>` preserving source order
- Verify that an absent `state` attribute is interpreted as `SheetVisibility::Visible`
- Verify the interpretation of `state="hidden"` / `state="veryHidden"`
- Verify that an unrecognized `state` value (e.g. a string from a future spec extension or a corrupted file) falls back to `SheetVisibility::Visible` rather than erroring
- Verify that a `<sheet>` missing `name` or `r:id` returns `Error::MissingRequiredElement`
- Verify that a `workbook.xml` with no `<sheets>` element at all returns `Error::MissingRequiredElement`
- Verify that an empty `<sheets></sheets>` produces an empty `Vec` (wiring for a zero-sheet workbook, matching [model/workbook.md Testing Strategy](../model/workbook.en.md))
- **Verify `<workbookPr date1904="1"/>` resolves `date1904: true`, and that `date1904="0"`, `date1904="true"`, an absent `date1904` attribute, and a wholly absent `<workbookPr>` element resolve to the expected value each** (Issue #40)

## Open Questions

1. **Handling of the `sheetId` attribute**: `<sheet sheetId="1">` is parsed but currently discarded, since [`model::Sheet`](../model/sheet.en.md) has no corresponding field. Whether it needs to be retained for future round-tripping use, and where (added to `WorkbookSheetEntry` or kept elsewhere), is undecided.
2. ~~Namespace resolution for `r:id`~~ → **Resolved**: follows the policy [parse/mod.md Open Question 4](mod.en.md) settled on — plain string-prefix matching, no `quick_xml::NsReader` — matching directly against the attribute name `"r:id"`.
3. **Soundness of falling back on an unrecognized `state` value**: currently falls back to `Visible`, but whether it should instead err on the safe side (treat it as `Hidden` so nothing is accidentally shown) is undecided pending actual requirements and use cases.
