# `parse/styles.rs` Design Doc

*[日本語](styles.md)*

Design doc for `src/parse/styles.rs`. Per [architecture.md](../architecture.en.md), this implements the `parse/` responsibility "parsing `styles.xml` (fonts/fills/borders/numFmts/cellXfs)." It parses `xl/styles.xml` and builds the `StyleSheet` defined by [`model/style.rs`](../model/style.en.md) (the table from `cellXfs` index to `ResolvedStyle`). This file also settles the open question left dangling by both [model/style.md Open Question 2](../model/style.en.md) and [resolve/style.md Open Question 2](../resolve/style.en.md): "where the date/time format-classification logic lives."

## Responsibility / Scope

- Parses `<numFmts>` (custom numeric-format definitions) and `<cellXfs>` (the array of format definitions actually applied to cells; its index matches [`model::style::StyleId`](../model/style.en.md))
- Resolves each `<xf>` in `<cellXfs>` by looking up the `numFmtId` it references — either a built-in format ID (fixed semantics in the 0–163 range) or a custom format defined in `<numFmts>` — and classifies whether that format represents a date/time (`ResolvedStyle::is_date_time`)
- Assigns `StyleId` in `<cellXfs>`'s index order and builds [`model::style::StyleSheet`](../model/style.en.md) (`HashMap<StyleId, Arc<ResolvedStyle>>`)
- **Not responsible for**: applying a `ResolvedStyle` to a cell ([`resolve/style.rs`](../resolve/style.en.md)), defining the `ResolvedStyle` / `StyleSheet` / `StyleId` types themselves ([`model/style.rs`](../model/style.en.md)), extracting visual style elements such as font, fill, or border (since `ResolvedStyle` currently only carries `is_date_time`, per [model/style.md Open Question 1](../model/style.en.md) still being unresolved, this file also skips over those XML elements)

## Key Types / Functions (draft)

```rust
use crate::error::Error;
use crate::model::style::{ResolvedStyle, StyleId, StyleSheet};
use crate::parse::{convert_xml_error, create_secure_reader, required_attr};
use std::collections::HashMap;
use std::io::BufRead;
use std::sync::Arc;

/// The built-in numFmtIds (ECMA-376 Part 1 §18.8.30) that represent a
/// date/time. 14–22: built-in date/time formats (e.g. 14 = "mm-dd-yy").
/// 45–47: elapsed time (e.g. 46 = "[h]:mm:ss"). Locale-dependent date
/// formats in the 27–36 range, including Japanese era (wareki) dates, are
/// not handled — see Open Question 1.
const BUILTIN_DATE_TIME_NUMFMT_IDS: &[u32] = &[14, 15, 16, 17, 18, 19, 20, 21, 22, 45, 46, 47];

/// Parses `xl/styles.xml` and builds a `StyleSheet`.
pub(crate) fn parse_styles(reader: impl BufRead, path: &str) -> Result<StyleSheet, Error> {
    let mut xml_reader = create_secure_reader(reader);
    // Implementation plan:
    // 1. Read <numFmts> first, building a numFmtId -> formatCode map (OOXML
    //    conventionally places <numFmts> before <cellXfs>, but the schema
    //    does not guarantee this ordering, so a two-pass read may be needed
    //    — see Open Question 4).
    // 2. For each <xf> in <cellXfs>, read the numFmtId attribute (defaults
    //    to 0 = General when absent), classify it via is_date_time_format,
    //    and build a ResolvedStyle.
    // 3. Store each into StyleSheet keyed by its 0-based index within
    //    <cellXfs>, used directly as StyleId.
    let mut num_fmts: HashMap<u32, String> = HashMap::new();
    let mut stylesheet: StyleSheet = HashMap::new();
    let _ = (&mut xml_reader, path, &mut num_fmts, &mut stylesheet);
    unimplemented!()
}

/// Classifies whether the format identified by `numfmt_id` — and, for a
/// custom format, `format_code` (the lookup result from `num_fmts`, `None`
/// if not found) — represents a date/time.
///
/// - `numfmt_id < 164` (built-in): checked against `BUILTIN_DATE_TIME_NUMFMT_IDS`.
/// - `numfmt_id >= 164` (custom): scans `format_code` heuristically for
///   date/time tokens (`y`, `m`, `d`, `h`, `s`, etc.), excluding `\`-escaped
///   or quoted literal characters and bracketed conditional-format segments
///   like `[Red]`. This classification is not exhaustive — see Open Question 2.
/// - If `numfmt_id` is found neither among the built-ins nor in the custom
///   definitions, falls back to `is_date_time: false` rather than erroring
///   — see Error Handling Policy.
fn is_date_time_format(numfmt_id: u32, format_code: Option<&str>) -> bool {
    let _ = (numfmt_id, format_code);
    unimplemented!()
}
```

## Dependencies

- Depends on: [`parse/mod.rs`](mod.en.md) (`create_secure_reader`, `convert_xml_error`, `required_attr`), [`model/style.rs`](../model/style.en.md) (`ResolvedStyle`, `StyleId`, `StyleSheet`), [`error.rs`](../error.en.md)
- Depended on by: [`resolve/style.rs`](../resolve/style.en.md) (looks up the built `StyleSheet` to apply to cells), `pipeline.rs` (built once between Phases 1–3 and passed to every `resolve_sheet` call; per architecture.md — "`StyleSheet` is discarded once Phase 4 completes" — dropped once every sheet has finished resolving)

This directly implements what [model/style.md Dependencies](../model/style.en.md) already committed to: "both `resolve/` and `parse/` depend only on `model/style.rs`, with no direct dependency on each other." This file (the builder) and `resolve/style.rs` (the applier) never know about each other — they connect only indirectly, through the shared vocabulary `StyleSheet` provides.

## Error Handling Policy

- Structurally broken `<numFmts>` / `<cellXfs>` XML (a syntax error) is converted into `Error::XmlParse` or `Error::ZipBombDetected` via [`convert_xml_error`](mod.en.md)
- An `<xf>` with no `numFmtId` attribute is treated as the default value `0` (`"General"`, not a date) — this is not `Error::MissingRequiredElement`, since `numFmtId` is an optional attribute per OOXML
- **When `numFmtId` is found neither among the built-in IDs nor in a custom `<numFmts>` definition, this falls back to `is_date_time: false` rather than erroring.** This extends the principle [resolve/style.md Error Handling Policy](../resolve/style.en.md) already adopted — "a loose failure interpreting an individual value is not an error unless it compromises the whole document's integrity" — prioritizing graceful degradation that reads as far into a broken or non-standard `styles.xml` as possible (an alternative is weighed in Open Question 3)
- The real-world impact of a wrong heuristic date/time classification (false positive or false negative) for custom formats is limited: a false positive (attempting to convert a non-date into DateTime) is further mitigated by [resolve/style.md](../resolve/style.en.md)'s `serial_to_date_time` fallback for unconvertible values (`CellValue::Number` is kept); a false negative (keeping a date as Number) never loses the cell's own value

## Testing Strategy

- Verify that an `<xf>` referencing a built-in `numFmtId` (e.g. `14` = `"mm-dd-yy"`) resolves to `is_date_time: true`
- Verify that a built-in `numFmtId` that is not date/time-related (e.g. `0` = `"General"`, `9` = `"0%"`) resolves to `is_date_time: false`
- Verify that a custom format (`numFmtId >= 164`) with a `<numFmts>` definition like `formatCode="yyyy/mm/dd"` resolves to `is_date_time: true`
- Verify that a custom format whose `formatCode` contains no date/time (e.g. `"#,##0.00"`, `"@"`) resolves to `is_date_time: false`
- Verify that a custom `formatCode` containing conditional-format sections or escaped characters (e.g. `"[Red]#,##0;[Blue]-#,##0"`) is not misclassified as `is_date_time: true` by false-positive detection of date-related tokens (a regression test for the heuristic's precision)
- Verify that a `numFmtId` found neither among built-ins nor custom definitions falls back to `is_date_time: false` without returning an error
- Verify that an `<xf>` with no `numFmtId` attribute is treated as the default `0` (`General`, not a date)
- Verify that the `StyleSheet` keys (`StyleId`) built from multiple `<xf>` entries in `<cellXfs>` match their 0-based index order within `<cellXfs>` (wiring with [resolve/style.md](../resolve/style.en.md))
- Verify correct resolution even for a `styles.xml` where `<numFmts>` appears after `<cellXfs>` (an ordering the XML schema permits) — a regression test against Open Question 4's implementation approach

## Open Questions

1. **Whether to support locale-dependent date formats including Japanese era (wareki) dates (`numFmtId` 27–36, etc.)**: since the requirements center on "Japanese business systems," whether to support custom date formats including the Japanese era (Reiwa, etc.) is to be settled together with a more detailed requirements pass.
2. **Precision of the custom `formatCode` date/time-classification heuristic**: whether bracketed conditional-format sections and quote/`\`-escaped literal characters can be reliably excluded is left to implementation-time detail design. As noted in Error Handling Policy, the real-world impact of a misclassification is limited, but there remains room to improve precision itself.
3. **Fallback vs. hard error for an undefined `numFmtId` reference**: currently assumes a graceful-degradation policy — reading as far as possible into an inaccurate or broken `styles.xml` — but there is a case for treating this internal reference inconsistency within `styles.xml` as a hard error too, for consistency with [resolve/style.md](../resolve/style.en.md)'s `Error::InvalidStyleId` (when a cell's own `cellXfs` index is itself invalid).
4. **Read order for `<numFmts>` and `<cellXfs>`**: `<numFmts>` conventionally appears before `<cellXfs>` in OOXML, but it is unconfirmed whether the schema strictly enforces this ordering. Whether to complete this in a single streaming pass (which would require deferring `<cellXfs>`'s resolution if `<numFmts>` appears later) or simply do a two-pass read is to be settled at implementation time.
5. **Concrete style elements such as font/fill/border**: same open topic as [model/style.md Open Question 1](../model/style.en.md) (unresolved). How far into cell styling (font color, background color, borders, bold/italic, etc.) the JSON output needs to go is to be settled together with `json.rs`'s design, or a more detailed requirements pass.
6. **Support for `applyNumberFormat` and `cellStyleXfs` (named cell-style inheritance)**: currently assumes a simplification where `numFmtId` is treated as authoritative regardless of an `<xf>`'s `applyNumberFormat` value, with no consideration of the inheritance chain from `cellStyleXfs` that `xfId` points to. Whether this simplification is sufficient within the requirements' scope is undecided.
