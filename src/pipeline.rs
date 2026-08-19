// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Orchestrates Phases 1-4 of the pipeline: wires together `container/`,
//! `parse/`, `resolve/`, and `model/` in call order, and controls resource
//! lifetimes (see `docs/design/architecture.en.md` design principle 3).

use crate::container::sanitize::SizeLimits;
use crate::container::ZipContainer;
use crate::error::Error;
use crate::model::{Sheet, SheetVisibility, StyleSheet, Workbook};
use crate::parse::SharedStringTable;
use crate::{parse, resolve};
use std::io::{BufReader, Read, Seek};

const WORKBOOK_RELS_PATH: &str = "xl/_rels/workbook.xml.rels";
const WORKBOOK_PATH: &str = "xl/workbook.xml";
const SHARED_STRINGS_REL_TYPE_SUFFIX: &str = "/relationships/sharedStrings";
const STYLES_REL_TYPE_SUFFIX: &str = "/relationships/styles";

/// One sheet's routing info, finalized once Phase 1 completes.
struct SheetRoute {
    name: String,
    visibility: SheetVisibility,
    /// A ZIP-entry-name-equivalent absolute path, ready to pass directly to
    /// `container::get_entry`.
    worksheet_path: String,
}

/// Runs Phases 1 through 4 end to end and returns the fully resolved
/// `Workbook`. `lib.rs`'s public API (`parse_workbook`, etc.) calls this
/// function. Generic over `Read + Seek` since it simply carries
/// forward `container::ZipContainer::open_reader`'s constraint (reading the
/// ZIP central directory requires a seekable input). `limits` is the Zip
/// Bomb size cap; `lib.rs`'s default-cap functions pass
/// `SizeLimits::default()`, while its `_with_limits` functions pass the
/// caller-supplied value straight through.
pub(crate) fn run<R: Read + Seek>(reader: R, limits: SizeLimits) -> Result<Workbook, Error> {
    let mut container = ZipContainer::open_reader(reader)?
        .with_max_entry_size(limits.max_entry_size)
        .with_max_total_size(limits.max_total_size);

    // --- Phase 1: relationship resolution and building the routing plan ---
    let rels_reader = container
        .get_entry(WORKBOOK_RELS_PATH)?
        .ok_or_else(|| Error::MissingRelationshipPart(WORKBOOK_RELS_PATH.to_string()))?;
    let relationships =
        parse::parse_relationships(BufReader::new(rels_reader), "xl", WORKBOOK_RELS_PATH)?;

    let workbook_reader = container
        .get_entry(WORKBOOK_PATH)?
        .ok_or_else(|| Error::InvalidPackage(WORKBOOK_PATH.to_string()))?;
    let parsed_workbook =
        parse::parse_workbook_xml(BufReader::new(workbook_reader), WORKBOOK_PATH)?;
    let date1904 = parsed_workbook.date1904;

    let mut routes = Vec::with_capacity(parsed_workbook.sheets.len());
    for entry in parsed_workbook.sheets {
        let rel = relationships
            .get(&entry.r_id)
            .ok_or_else(|| Error::DanglingRelationship {
                r_id: entry.r_id.clone(),
            })?;
        routes.push(SheetRoute {
            name: entry.name,
            visibility: entry.visibility,
            worksheet_path: rel.target.clone(),
        });
    }
    let shared_strings_path = relationships
        .values()
        .find(|r| r.rel_type.ends_with(SHARED_STRINGS_REL_TYPE_SUFFIX))
        .map(|r| r.target.clone());
    // A workbook that applies no cell styling at all is not required by
    // OOXML to carry a styles.xml part (Issue #54) — real third-party
    // writers have been observed to omit it entirely, and both Excel and
    // other readers accept such files by falling back to no styling rather
    // than rejecting the package.
    let styles_path = relationships
        .values()
        .find(|r| r.rel_type.ends_with(STYLES_REL_TYPE_SUFFIX))
        .map(|r| r.target.clone());

    // The reader used for the rels read, and the RelationshipMap, go out of
    // scope and are dropped here (implements architecture.md's "dispose of
    // the _rels scratch buffer at the end of Phase 1").
    drop(relationships);

    // --- Shared tables, built exactly once between Phases 1-3 ---
    let shared_string_table = match shared_strings_path {
        Some(path) => {
            let reader = container
                .get_entry(&path)?
                .ok_or_else(|| Error::InvalidPackage(path.clone()))?;
            parse::parse_shared_strings(BufReader::new(reader), &path)?
        }
        // sharedStrings.xml itself is an optional OOXML part (may be
        // omitted for a workbook with no string cells at all).
        None => SharedStringTable::default(),
    };
    let stylesheet = match styles_path {
        Some(path) => {
            let reader = container
                .get_entry(&path)?
                .ok_or_else(|| Error::InvalidPackage(path.clone()))?;
            parse::parse_styles(BufReader::new(reader), &path)?
        }
        // The relationship itself is absent — genuinely no styles.xml part
        // (as opposed to a relationship pointing at a missing entity, which
        // stays Error::InvalidPackage above). No cell can reference a
        // StyleId that was never assigned, so an empty StyleSheet degrades
        // gracefully rather than erroring.
        None => StyleSheet::new(),
    };

    // --- Per sheet: Phase 3 (streaming parse) -> Phase 4 (resolution) ---
    let mut sheets = Vec::with_capacity(routes.len());
    for route in routes {
        let mut sheet = Sheet::new(route.name, route.visibility);
        let reader = container.get_entry(&route.worksheet_path)?.ok_or_else(|| {
            Error::DanglingRelationship {
                r_id: route.worksheet_path.clone(),
            }
        })?;
        let output =
            parse::parse_worksheet(BufReader::new(reader), &route.worksheet_path, &mut sheet)?;

        // --- Phase 3.5: image anchor resolution (Issue #65), only if this
        // sheet declared a <drawing r:id>. Kept in pipeline.rs rather than
        // resolve/, since every step here does ZIP I/O and resolve/'s
        // I/O-independence (architecture.md design principle 2) must not be
        // compromised. ---
        if let Some(drawing_r_id) = &output.drawing_r_id {
            let images = resolve_sheet_images(&mut container, &route.worksheet_path, drawing_r_id)?;
            sheet.set_images(images);
        }

        resolve::resolve_sheet(
            &mut sheet,
            &output.pending_shared_strings,
            &shared_string_table,
            &output.pending_styles,
            &stylesheet,
            date1904,
            output.col_width_ranges,
            output.default_col_width,
            output.merge_regions,
        )?;
        sheets.push(sheet);
    }
    // shared_string_table / stylesheet go out of scope and are dropped here
    // (implements architecture.md's "dispose of SharedStringTable and
    // StyleSheet once Phase 4 completes").

    Ok(Workbook::new(sheets))
}

/// Computes the `_rels` part path for a given OPC part (e.g.
/// `"xl/worksheets/sheet1.xml"` -> `"xl/worksheets/_rels/sheet1.xml.rels"`),
/// alongside the part's own directory (`"xl/worksheets"`) — the anchor
/// `parse::parse_relationships` needs to resolve that rels part's relative
/// `Target` paths. Generalizes the pattern `WORKBOOK_RELS_PATH`/`"xl"`
/// already hard-codes for the one workbook-level rels file, to any per-part
/// rels file (Issue #65: needed for both the worksheet's own rels, which
/// locates `drawingN.xml`, and that drawing's rels, which locates the
/// embedded media).
fn rels_path_for(part_path: &str) -> (String, &str) {
    match part_path.rfind('/') {
        Some(idx) => {
            let dir = &part_path[..idx];
            let file_name = &part_path[idx + 1..];
            (format!("{dir}/_rels/{file_name}.rels"), dir)
        }
        None => (format!("_rels/{part_path}.rels"), ""),
    }
}

/// Phase 3.5: resolves a worksheet's `<drawing r:id>` all the way down to a
/// `Vec<model::Image>` — locating `drawingN.xml` via the worksheet's own
/// `_rels`, parsing it (`parse::parse_drawing`), then resolving each
/// `<xdr:pic>`'s `r:embed`/hyperlink `r:id` against `drawingN.xml`'s own
/// `_rels`.
///
/// Error handling mirrors how `route.worksheet_path` itself is resolved
/// above: any `r:id` referenced by the XML that fails to resolve — whether
/// because the `_rels` part doesn't exist at all, the `r:id` isn't in it, or
/// the target it resolves to has no matching ZIP entry — is
/// `Error::DanglingRelationship`, fail-fast. This sheet already declared
/// `<drawing r:id="...">`, so an unresolvable reference here means a
/// genuinely broken package, not an absent optional feature (contrast the
/// sharedStrings/styles relationship-type-absent case above, which
/// legitimately falls back to an empty default instead).
///
/// Only an `Internal` (in-package) target's existence is verified against
/// the ZIP; an `External` one (a hyperlink URL, most commonly) is kept
/// as-is without a filesystem check, since this library never fetches
/// external resources. Neither branch ever reads the target's bytes — only
/// `get_entry`'s `Option` is inspected — so resolving images costs O(image
/// count), never O(image size).
fn resolve_sheet_images<R: Read + Seek>(
    container: &mut ZipContainer<R>,
    worksheet_path: &str,
    drawing_r_id: &str,
) -> Result<Vec<crate::model::Image>, Error> {
    let (worksheet_rels_path, worksheet_dir) = rels_path_for(worksheet_path);
    let worksheet_rels_reader =
        container
            .get_entry(&worksheet_rels_path)?
            .ok_or_else(|| Error::DanglingRelationship {
                r_id: drawing_r_id.to_string(),
            })?;
    let worksheet_rels = parse::parse_relationships(
        BufReader::new(worksheet_rels_reader),
        worksheet_dir,
        &worksheet_rels_path,
    )?;
    let drawing_path = worksheet_rels
        .get(drawing_r_id)
        .ok_or_else(|| Error::DanglingRelationship {
            r_id: drawing_r_id.to_string(),
        })?
        .target
        .clone();

    let drawing_reader =
        container
            .get_entry(&drawing_path)?
            .ok_or_else(|| Error::DanglingRelationship {
                r_id: drawing_path.clone(),
            })?;
    let pending_images = parse::parse_drawing(BufReader::new(drawing_reader), &drawing_path)?;
    if pending_images.is_empty() {
        return Ok(Vec::new());
    }

    let (drawing_rels_path, drawing_dir) = rels_path_for(&drawing_path);
    let drawing_rels = match container.get_entry(&drawing_rels_path)? {
        Some(reader) => Some(parse::parse_relationships(
            BufReader::new(reader),
            drawing_dir,
            &drawing_rels_path,
        )?),
        None => None,
    };

    let mut images = Vec::with_capacity(pending_images.len());
    for pending in pending_images {
        let rels = drawing_rels
            .as_ref()
            .ok_or_else(|| Error::DanglingRelationship {
                r_id: pending.embed_r_id.clone(),
            })?;

        let embed_rel =
            rels.get(&pending.embed_r_id)
                .ok_or_else(|| Error::DanglingRelationship {
                    r_id: pending.embed_r_id.clone(),
                })?;
        if embed_rel.target_mode == parse::TargetMode::Internal
            && container.get_entry(&embed_rel.target)?.is_none()
        {
            return Err(Error::DanglingRelationship {
                r_id: pending.embed_r_id.clone(),
            });
        }
        let target = embed_rel.target.clone();

        let hyperlink = match &pending.hyperlink_r_id {
            Some(hyperlink_r_id) => {
                let hyperlink_rel =
                    rels.get(hyperlink_r_id)
                        .ok_or_else(|| Error::DanglingRelationship {
                            r_id: hyperlink_r_id.clone(),
                        })?;
                if hyperlink_rel.target_mode == parse::TargetMode::Internal
                    && container.get_entry(&hyperlink_rel.target)?.is_none()
                {
                    return Err(Error::DanglingRelationship {
                        r_id: hyperlink_r_id.clone(),
                    });
                }
                Some(hyperlink_rel.target.clone())
            }
            None => None,
        };

        images.push(crate::model::Image {
            anchor: pending.anchor,
            target,
            hyperlink,
        });
    }

    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CellRef, CellValue};
    use std::io::{Cursor, Write};

    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buf));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, data) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buf
    }

    const RELS_XML: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#;

    const WORKBOOK_XML: &[u8] = br#"<?xml version="1.0"?>
<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
    <sheet name="Sheet1" sheetId="1" r:id="rId1"/>
  </sheets>
</workbook>"#;

    const SHARED_STRINGS_XML: &[u8] = b"<sst><si><t>hello</t></si></sst>";

    const STYLES_XML: &[u8] = br#"<styleSheet><cellXfs><xf numFmtId="0"/></cellXfs></styleSheet>"#;

    const WORKSHEET_XML: &[u8] = br#"<worksheet><sheetData>
<row r="1">
  <c r="A1"><v>42</v></c>
  <c r="B1" t="s"><v>0</v></c>
</row>
</sheetData>
<mergeCells count="1"><mergeCell ref="C1:D1"/></mergeCells>
</worksheet>"#;

    fn minimal_xlsx() -> Vec<u8> {
        build_zip(&[
            ("xl/_rels/workbook.xml.rels", RELS_XML),
            ("xl/workbook.xml", WORKBOOK_XML),
            ("xl/sharedStrings.xml", SHARED_STRINGS_XML),
            ("xl/styles.xml", STYLES_XML),
            ("xl/worksheets/sheet1.xml", WORKSHEET_XML),
        ])
    }

    #[test]
    fn minimal_valid_xlsx_resolves_end_to_end() {
        let workbook = run(Cursor::new(minimal_xlsx()), SizeLimits::default()).unwrap();

        assert_eq!(workbook.sheets().len(), 1);
        let sheet = &workbook.sheets()[0];
        assert_eq!(sheet.name, "Sheet1");
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 1 }).unwrap().value,
            Some(CellValue::Number(42.0))
        );
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 2 }).unwrap().value,
            Some(CellValue::Text(std::sync::Arc::from("hello")))
        );
        // The merged range C1:D1 makes D1 resolve to the same cell as C1.
        assert_eq!(
            sheet.get(CellRef { row: 1, col: 3 }),
            sheet.get(CellRef { row: 1, col: 4 })
        );
    }

    #[test]
    fn cell_ref_beyond_excels_real_maximum_is_invalid_cell_ref() {
        // Security review docs/security/code-review.md Finding 2, exercised
        // end to end through real worksheet XML rather than a direct
        // CellRef::from_a1 call.
        let sheet_with_forged_coordinate: &[u8] =
            br#"<worksheet><sheetData><row r="1"><c r="ZZZZZZ4294967294"><v>1</v></c></row></sheetData></worksheet>"#;
        let zip = build_zip(&[
            ("xl/_rels/workbook.xml.rels", RELS_XML),
            ("xl/workbook.xml", WORKBOOK_XML),
            ("xl/sharedStrings.xml", SHARED_STRINGS_XML),
            ("xl/styles.xml", STYLES_XML),
            ("xl/worksheets/sheet1.xml", sheet_with_forged_coordinate),
        ]);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::InvalidCellRef(_)));
    }

    #[test]
    fn package_absolute_relationship_target_resolves_like_openpyxl_output() {
        // openpyxl writes the worksheet relationship as a package-absolute
        // target (Target="/xl/worksheets/sheet1.xml") while leaving styles/
        // sharedStrings relative — this exact mixed pattern previously made
        // every openpyxl-produced .xlsx fail to parse (see
        // parse::relationships::resolve_target_path's regression test for
        // the unit-level case; this proves the whole pipeline handles it).
        let rels_with_absolute_worksheet_target: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="/xl/worksheets/sheet1.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#;
        let zip = build_zip(&[
            (
                "xl/_rels/workbook.xml.rels",
                rels_with_absolute_worksheet_target,
            ),
            ("xl/workbook.xml", WORKBOOK_XML),
            ("xl/sharedStrings.xml", SHARED_STRINGS_XML),
            ("xl/styles.xml", STYLES_XML),
            ("xl/worksheets/sheet1.xml", WORKSHEET_XML),
        ]);
        let workbook = run(Cursor::new(zip), SizeLimits::default()).unwrap();
        assert_eq!(workbook.sheets().len(), 1);
    }

    #[test]
    fn excessive_merge_cell_count_is_too_many_merged_ranges() {
        // Security review docs/security/code-review.md Finding 1: without
        // resolve::merge's MAX_MERGE_REGIONS cap, a sheet with a large
        // number of non-overlapping <mergeCell> entries costs O(N^2) to
        // validate, letting a file of a few hundred KB block the caller
        // for minutes. 20,001 (one over the cap) is used here rather than
        // the exact constant to keep this an end-to-end, XML-driven check
        // independent of resolve::merge's private module path; the
        // boundary itself is covered precisely by
        // resolve::merge::tests::region_count_over_the_limit_is_too_many_merged_ranges.
        let mut merge_cells = String::from("<mergeCells count=\"20001\">");
        for i in 1..=20_001u32 {
            merge_cells.push_str(&format!("<mergeCell ref=\"A{i}:B{i}\"/>"));
        }
        merge_cells.push_str("</mergeCells>");
        let sheet_with_excessive_merges =
            format!("<worksheet><sheetData></sheetData>{merge_cells}</worksheet>");

        let zip = build_zip(&[
            ("xl/_rels/workbook.xml.rels", RELS_XML),
            ("xl/workbook.xml", WORKBOOK_XML),
            ("xl/sharedStrings.xml", SHARED_STRINGS_XML),
            ("xl/styles.xml", STYLES_XML),
            (
                "xl/worksheets/sheet1.xml",
                sheet_with_excessive_merges.as_bytes(),
            ),
        ]);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::TooManyMergedRanges { .. }));
    }

    #[test]
    fn excessive_col_width_range_count_is_too_many_column_width_ranges() {
        // Issue #39, same reasoning as the merge-count cap above: an
        // unbounded number of tiny <col> entries fits comfortably within
        // the Zip Bomb byte cap, so the range count itself must be bounded
        // independently. 2,001 (one over
        // resolve::column_width::MAX_COLUMN_WIDTH_RANGES) is used here
        // rather than importing the private constant, to keep this an
        // end-to-end, XML-driven check.
        let mut cols = String::from("<cols>");
        for i in 1..=2_001u32 {
            cols.push_str(&format!("<col min=\"{i}\" max=\"{i}\" width=\"10\"/>"));
        }
        cols.push_str("</cols>");
        let sheet_with_excessive_col_widths =
            format!("<worksheet>{cols}<sheetData></sheetData></worksheet>");

        let zip = build_zip(&[
            ("xl/_rels/workbook.xml.rels", RELS_XML),
            ("xl/workbook.xml", WORKBOOK_XML),
            ("xl/sharedStrings.xml", SHARED_STRINGS_XML),
            ("xl/styles.xml", STYLES_XML),
            (
                "xl/worksheets/sheet1.xml",
                sheet_with_excessive_col_widths.as_bytes(),
            ),
        ]);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::TooManyColumnWidthRanges { .. }));
    }

    #[test]
    fn full_width_single_col_range_registers_without_hanging() {
        // Mirrors model::sheet::tests::insert_merge_on_huge_region_does_not_hang
        // for column widths: a single <col min="1" max="16384" .../> must
        // register as one range, not expand into 16,384 entries.
        let sheet_with_full_width_col = r#"<worksheet>
<cols><col min="1" max="16384" width="8.43"/></cols>
<sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData>
</worksheet>"#;

        let zip = build_zip(&[
            ("xl/_rels/workbook.xml.rels", RELS_XML),
            ("xl/workbook.xml", WORKBOOK_XML),
            ("xl/sharedStrings.xml", SHARED_STRINGS_XML),
            ("xl/styles.xml", STYLES_XML),
            (
                "xl/worksheets/sheet1.xml",
                sheet_with_full_width_col.as_bytes(),
            ),
        ]);
        let workbook = run(Cursor::new(zip), SizeLimits::default()).unwrap();
        let sheet = &workbook.sheets()[0];
        assert_eq!(sheet.column_width(1), Some(8.43));
        assert_eq!(sheet.column_width(16_384), Some(8.43));
    }

    #[test]
    fn caller_supplied_size_limits_are_forwarded_to_the_container() {
        // Succeeds under the default limits...
        run(Cursor::new(minimal_xlsx()), SizeLimits::default()).unwrap();

        // ...but a caller-supplied max_entry_size small enough to reject
        // xl/workbook.xml turns the same input into Error::ZipBombDetected,
        // proving `limits` actually reaches `ZipContainer` rather than being
        // silently ignored in favor of the DEFAULT_MAX_* constants.
        let tiny_limits = SizeLimits {
            max_entry_size: 1,
            max_total_size: SizeLimits::default().max_total_size,
        };
        let err = run(Cursor::new(minimal_xlsx()), tiny_limits).unwrap_err();
        assert!(matches!(err, Error::ZipBombDetected { .. }));
    }

    #[test]
    fn missing_workbook_rels_is_missing_relationship_part() {
        let zip = build_zip(&[("xl/workbook.xml", WORKBOOK_XML)]);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::MissingRelationshipPart(_)));
    }

    #[test]
    fn missing_workbook_xml_is_invalid_package() {
        let zip = build_zip(&[("xl/_rels/workbook.xml.rels", RELS_XML)]);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::InvalidPackage(_)));
    }

    #[test]
    fn dangling_sheet_relationship_is_dangling_relationship() {
        let rels_without_sheet_rel: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#;
        let zip = build_zip(&[
            ("xl/_rels/workbook.xml.rels", rels_without_sheet_rel),
            ("xl/workbook.xml", WORKBOOK_XML),
            ("xl/styles.xml", STYLES_XML),
        ]);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::DanglingRelationship { .. }));
    }

    #[test]
    fn missing_styles_relationship_falls_back_to_empty_stylesheet() {
        // Issue #54: no relationship of type .../relationships/styles at
        // all — the genuine "this OOXML part is entirely absent" case,
        // distinct from missing_styles_entity_is_invalid_package below
        // (where the relationship exists but the target entity doesn't).
        let rels_without_styles: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
</Relationships>"#;
        let plain_sheet: &[u8] = br#"<worksheet><sheetData>
<row r="1"><c r="A1"><v>1</v></c></row>
</sheetData></worksheet>"#;
        let zip = build_zip(&[
            ("xl/_rels/workbook.xml.rels", rels_without_styles),
            ("xl/workbook.xml", WORKBOOK_XML),
            ("xl/worksheets/sheet1.xml", plain_sheet),
        ]);
        let workbook = run(Cursor::new(zip), SizeLimits::default()).unwrap();
        assert_eq!(
            workbook.sheets()[0]
                .get(CellRef { row: 1, col: 1 })
                .unwrap()
                .value,
            Some(CellValue::Number(1.0))
        );
    }

    #[test]
    fn missing_styles_entity_is_invalid_package() {
        let zip = build_zip(&[
            ("xl/_rels/workbook.xml.rels", RELS_XML),
            ("xl/workbook.xml", WORKBOOK_XML),
            ("xl/sharedStrings.xml", SHARED_STRINGS_XML),
            // xl/styles.xml intentionally omitted, even though rels points to it.
            ("xl/worksheets/sheet1.xml", WORKSHEET_XML),
        ]);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::InvalidPackage(_)));
    }

    #[test]
    fn missing_worksheet_entity_is_dangling_relationship() {
        let zip = build_zip(&[
            ("xl/_rels/workbook.xml.rels", RELS_XML),
            ("xl/workbook.xml", WORKBOOK_XML),
            ("xl/sharedStrings.xml", SHARED_STRINGS_XML),
            ("xl/styles.xml", STYLES_XML),
            // xl/worksheets/sheet1.xml intentionally omitted.
        ]);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::DanglingRelationship { .. }));
    }

    #[test]
    fn missing_shared_strings_part_falls_back_to_empty_table() {
        // No relationship of type .../relationships/sharedStrings at all —
        // the genuine "this OOXML part is entirely absent" case. (Merely
        // omitting the sharedStrings.xml *entry* while keeping a
        // relationship that points to it is a different, dangling-part
        // case that correctly errors as Error::InvalidPackage instead.)
        let rels_without_shared_strings: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#;
        let workbook_no_strings: &[u8] = br#"<worksheet><sheetData>
<row r="1"><c r="A1"><v>1</v></c></row>
</sheetData></worksheet>"#;
        let zip = build_zip(&[
            ("xl/_rels/workbook.xml.rels", rels_without_shared_strings),
            ("xl/workbook.xml", WORKBOOK_XML),
            ("xl/styles.xml", STYLES_XML),
            ("xl/worksheets/sheet1.xml", workbook_no_strings),
        ]);
        let workbook = run(Cursor::new(zip), SizeLimits::default()).unwrap();
        assert_eq!(workbook.sheets().len(), 1);
    }

    #[test]
    fn multiple_sheets_preserve_definition_order() {
        let workbook_two_sheets: &[u8] = br#"<?xml version="1.0"?>
<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
    <sheet name="First" sheetId="1" r:id="rId1"/>
    <sheet name="Second" sheetId="2" r:id="rId4"/>
  </sheets>
</workbook>"#;
        let rels_two_sheets: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#;
        let blank_sheet: &[u8] = b"<worksheet><sheetData></sheetData></worksheet>";
        let zip = build_zip(&[
            ("xl/_rels/workbook.xml.rels", rels_two_sheets),
            ("xl/workbook.xml", workbook_two_sheets),
            ("xl/styles.xml", STYLES_XML),
            ("xl/worksheets/sheet1.xml", blank_sheet),
            ("xl/worksheets/sheet2.xml", blank_sheet),
        ]);
        let workbook = run(Cursor::new(zip), SizeLimits::default()).unwrap();
        let names: Vec<&str> = workbook.sheets().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["First", "Second"]);
    }

    #[test]
    fn second_sheet_failure_fails_the_whole_run() {
        let workbook_two_sheets: &[u8] = br#"<?xml version="1.0"?>
<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
    <sheet name="First" sheetId="1" r:id="rId1"/>
    <sheet name="Second" sheetId="2" r:id="rId4"/>
  </sheets>
</workbook>"#;
        let rels_two_sheets: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#;
        let good_sheet: &[u8] = b"<worksheet><sheetData></sheetData></worksheet>";
        // Missing `r` attribute on <c> makes Phase 3 fail for this sheet.
        let broken_sheet: &[u8] =
            br#"<worksheet><sheetData><row r="1"><c t="n"><v>1</v></c></row></sheetData></worksheet>"#;
        let zip = build_zip(&[
            ("xl/_rels/workbook.xml.rels", rels_two_sheets),
            ("xl/workbook.xml", workbook_two_sheets),
            ("xl/styles.xml", STYLES_XML),
            ("xl/worksheets/sheet1.xml", good_sheet),
            ("xl/worksheets/sheet2.xml", broken_sheet),
        ]);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::MissingRequiredElement { .. }));
    }

    #[test]
    fn hidden_and_very_hidden_sheets_are_all_included() {
        let workbook_hidden: &[u8] = br#"<?xml version="1.0"?>
<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
    <sheet name="Visible" sheetId="1" r:id="rId1"/>
    <sheet name="Hidden" sheetId="2" r:id="rId4" state="hidden"/>
    <sheet name="VeryHidden" sheetId="3" r:id="rId5" state="veryHidden"/>
  </sheets>
</workbook>"#;
        let rels_hidden: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>
  <Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet3.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#;
        let blank_sheet: &[u8] = b"<worksheet><sheetData></sheetData></worksheet>";
        let zip = build_zip(&[
            ("xl/_rels/workbook.xml.rels", rels_hidden),
            ("xl/workbook.xml", workbook_hidden),
            ("xl/styles.xml", STYLES_XML),
            ("xl/worksheets/sheet1.xml", blank_sheet),
            ("xl/worksheets/sheet2.xml", blank_sheet),
            ("xl/worksheets/sheet3.xml", blank_sheet),
        ]);
        let workbook = run(Cursor::new(zip), SizeLimits::default()).unwrap();
        assert_eq!(workbook.sheets().len(), 3);
    }

    const WORKSHEET_WITH_DRAWING_XML: &[u8] =
        br#"<worksheet><sheetData></sheetData><drawing r:id="rIdDrawing"/></worksheet>"#;

    const WORKSHEET_RELS_WITH_DRAWING: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdDrawing" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/>
</Relationships>"#;

    const DRAWING1_XML: &[u8] = br#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <xdr:twoCellAnchor>
    <xdr:from><xdr:col>1</xdr:col><xdr:colOff>10</xdr:colOff><xdr:row>2</xdr:row><xdr:rowOff>20</xdr:rowOff></xdr:from>
    <xdr:to><xdr:col>5</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>10</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>
    <xdr:pic>
      <xdr:nvPicPr>
        <xdr:cNvPr id="2" name="Picture 1">
          <a:hlinkClick r:id="rIdHyperlink"/>
        </xdr:cNvPr>
        <xdr:cNvPicPr/>
      </xdr:nvPicPr>
      <xdr:blipFill><a:blip r:embed="rIdEmbed"/></xdr:blipFill>
    </xdr:pic>
    <xdr:clientData/>
  </xdr:twoCellAnchor>
</xdr:wsDr>"#;

    const DRAWING1_RELS_XML: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdEmbed" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/>
  <Relationship Id="rIdHyperlink" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com" TargetMode="External"/>
</Relationships>"#;

    fn xlsx_with_drawing_entries() -> Vec<(&'static str, &'static [u8])> {
        vec![
            ("xl/_rels/workbook.xml.rels", RELS_XML),
            ("xl/workbook.xml", WORKBOOK_XML),
            ("xl/sharedStrings.xml", SHARED_STRINGS_XML),
            ("xl/styles.xml", STYLES_XML),
            ("xl/worksheets/sheet1.xml", WORKSHEET_WITH_DRAWING_XML),
            (
                "xl/worksheets/_rels/sheet1.xml.rels",
                WORKSHEET_RELS_WITH_DRAWING,
            ),
            ("xl/drawings/drawing1.xml", DRAWING1_XML),
            ("xl/drawings/_rels/drawing1.xml.rels", DRAWING1_RELS_XML),
            ("xl/media/image1.png", b"\x89PNG\r\n\x1a\n" as &[u8]),
        ]
    }

    #[test]
    fn image_anchor_resolves_end_to_end_with_embed_and_hyperlink() {
        use crate::model::{AnchorMarker, ImageAnchor};

        let zip = build_zip(&xlsx_with_drawing_entries());
        let workbook = run(Cursor::new(zip), SizeLimits::default()).unwrap();
        let images = workbook.sheets()[0].images();
        assert_eq!(images.len(), 1);
        let image = &images[0];
        assert_eq!(image.target, "xl/media/image1.png");
        assert_eq!(image.hyperlink.as_deref(), Some("https://example.com"));
        assert_eq!(
            image.anchor,
            ImageAnchor::TwoCell {
                from: AnchorMarker {
                    cell: CellRef { row: 3, col: 2 },
                    col_off: 10,
                    row_off: 20,
                },
                to: AnchorMarker {
                    cell: CellRef { row: 11, col: 6 },
                    col_off: 0,
                    row_off: 0,
                },
            }
        );
    }

    #[test]
    fn sheet_without_drawing_element_has_no_images() {
        let zip = build_zip(&minimal_xlsx_entries());
        let workbook = run(Cursor::new(zip), SizeLimits::default()).unwrap();
        assert!(workbook.sheets()[0].images().is_empty());
    }

    fn minimal_xlsx_entries() -> Vec<(&'static str, &'static [u8])> {
        vec![
            ("xl/_rels/workbook.xml.rels", RELS_XML),
            ("xl/workbook.xml", WORKBOOK_XML),
            ("xl/sharedStrings.xml", SHARED_STRINGS_XML),
            ("xl/styles.xml", STYLES_XML),
            ("xl/worksheets/sheet1.xml", WORKSHEET_XML),
        ]
    }

    #[test]
    fn drawing_r_id_with_no_worksheet_rels_part_is_dangling_relationship() {
        // <drawing r:id> is present in the worksheet XML, but no
        // xl/worksheets/_rels/sheet1.xml.rels part exists at all — a
        // genuinely broken package, not the "optional feature absent" case
        // (contrast missing_styles_relationship_falls_back_to_empty_stylesheet).
        let mut entries = xlsx_with_drawing_entries();
        entries.retain(|(name, _)| *name != "xl/worksheets/_rels/sheet1.xml.rels");
        let zip = build_zip(&entries);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::DanglingRelationship { .. }));
    }

    #[test]
    fn drawing_r_id_not_in_worksheet_rels_is_dangling_relationship() {
        let rels_without_the_drawing_id: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"></Relationships>"#;
        let mut entries = xlsx_with_drawing_entries();
        for entry in &mut entries {
            if entry.0 == "xl/worksheets/_rels/sheet1.xml.rels" {
                entry.1 = rels_without_the_drawing_id;
            }
        }
        let zip = build_zip(&entries);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::DanglingRelationship { .. }));
    }

    #[test]
    fn missing_drawing_entity_is_dangling_relationship() {
        let mut entries = xlsx_with_drawing_entries();
        entries.retain(|(name, _)| *name != "xl/drawings/drawing1.xml");
        let zip = build_zip(&entries);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::DanglingRelationship { .. }));
    }

    #[test]
    fn missing_embedded_media_entity_is_dangling_relationship() {
        // The rels correctly point to xl/media/image1.png, but the entry
        // itself doesn't exist in the ZIP.
        let mut entries = xlsx_with_drawing_entries();
        entries.retain(|(name, _)| *name != "xl/media/image1.png");
        let zip = build_zip(&entries);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::DanglingRelationship { .. }));
    }

    #[test]
    fn missing_drawing_rels_part_is_dangling_relationship() {
        // drawing1.xml itself parses fine (it has a <xdr:pic>), but there is
        // no xl/drawings/_rels/drawing1.xml.rels to resolve r:embed against.
        let mut entries = xlsx_with_drawing_entries();
        entries.retain(|(name, _)| *name != "xl/drawings/_rels/drawing1.xml.rels");
        let zip = build_zip(&entries);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::DanglingRelationship { .. }));
    }

    #[test]
    fn rels_path_for_with_no_directory_component() {
        // Every real OPC part path this crate ever computes has a
        // directory component (e.g. "xl/worksheets/sheet1.xml"), so this
        // exercises the degenerate case directly rather than via `run`.
        assert_eq!(
            rels_path_for("sheet1.xml"),
            ("_rels/sheet1.xml.rels".to_string(), "")
        );
    }

    #[test]
    fn malformed_worksheet_rels_xml_propagates_parse_error() {
        // Distinct from drawing_r_id_with_no_worksheet_rels_part_is_dangling_relationship
        // (the part is entirely absent): here the part exists but is
        // malformed XML, so parse::parse_relationships itself must fail and
        // that failure must propagate rather than being swallowed.
        let malformed_rels: &[u8] =
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdDrawing"/></Relationships>"#;
        let mut entries = xlsx_with_drawing_entries();
        for entry in &mut entries {
            if entry.0 == "xl/worksheets/_rels/sheet1.xml.rels" {
                entry.1 = malformed_rels;
            }
        }
        let zip = build_zip(&entries);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::MissingRequiredElement { .. }));
    }

    #[test]
    fn malformed_drawing_rels_xml_propagates_parse_error() {
        let malformed_rels: &[u8] =
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdEmbed"/></Relationships>"#;
        let mut entries = xlsx_with_drawing_entries();
        for entry in &mut entries {
            if entry.0 == "xl/drawings/_rels/drawing1.xml.rels" {
                entry.1 = malformed_rels;
            }
        }
        let zip = build_zip(&entries);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::MissingRequiredElement { .. }));
    }

    #[test]
    fn drawing_with_no_picture_anchors_yields_no_images() {
        // A <drawing> that resolves and parses fine but anchors a plain
        // shape, not a <xdr:pic> — parse::parse_drawing correctly yields no
        // PendingImage, and resolve_sheet_images must short-circuit before
        // ever trying to read drawing1.xml.rels or any media entry (neither
        // is present in this ZIP at all, proving the early return is real).
        let drawing_without_pic: &[u8] = br#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <xdr:twoCellAnchor>
    <xdr:from><xdr:col>0</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>0</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from>
    <xdr:to><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>
    <xdr:sp/>
    <xdr:clientData/>
  </xdr:twoCellAnchor>
</xdr:wsDr>"#;
        let mut entries = xlsx_with_drawing_entries();
        entries.retain(|(name, _)| {
            !matches!(
                *name,
                "xl/drawings/_rels/drawing1.xml.rels" | "xl/media/image1.png"
            )
        });
        for entry in &mut entries {
            if entry.0 == "xl/drawings/drawing1.xml" {
                entry.1 = drawing_without_pic;
            }
        }
        let zip = build_zip(&entries);
        let workbook = run(Cursor::new(zip), SizeLimits::default()).unwrap();
        assert!(workbook.sheets()[0].images().is_empty());
    }

    #[test]
    fn embed_r_id_not_in_drawing_rels_is_dangling_relationship() {
        // drawing1.xml.rels exists and parses fine, but has no entry for
        // rIdEmbed at all (distinct from missing_drawing_rels_part_is_dangling_relationship,
        // where the whole rels part is absent).
        let rels_without_embed: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdUnrelated" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/>
</Relationships>"#;
        let mut entries = xlsx_with_drawing_entries();
        for entry in &mut entries {
            if entry.0 == "xl/drawings/_rels/drawing1.xml.rels" {
                entry.1 = rels_without_embed;
            }
        }
        let zip = build_zip(&entries);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::DanglingRelationship { .. }));
    }

    #[test]
    fn hyperlink_r_id_not_in_drawing_rels_is_dangling_relationship() {
        // rIdEmbed resolves fine, but rIdHyperlink (referenced by
        // <a:hlinkClick> in DRAWING1_XML) has no entry in drawing1.xml.rels.
        let rels_without_hyperlink: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdEmbed" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/>
</Relationships>"#;
        let mut entries = xlsx_with_drawing_entries();
        for entry in &mut entries {
            if entry.0 == "xl/drawings/_rels/drawing1.xml.rels" {
                entry.1 = rels_without_hyperlink;
            }
        }
        let zip = build_zip(&entries);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::DanglingRelationship { .. }));
    }

    #[test]
    fn internal_hyperlink_target_missing_is_dangling_relationship() {
        // rIdHyperlink resolves to an Internal (in-package) relationship
        // this time, rather than DRAWING1_RELS_XML's External URL — its
        // target must be checked for existence in the ZIP just like an
        // embed target is, and this one doesn't exist.
        let rels_with_internal_hyperlink: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdEmbed" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/>
  <Relationship Id="rIdHyperlink" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="../other/missing.xml"/>
</Relationships>"#;
        let mut entries = xlsx_with_drawing_entries();
        for entry in &mut entries {
            if entry.0 == "xl/drawings/_rels/drawing1.xml.rels" {
                entry.1 = rels_with_internal_hyperlink;
            }
        }
        let zip = build_zip(&entries);
        let err = run(Cursor::new(zip), SizeLimits::default()).unwrap_err();
        assert!(matches!(err, Error::DanglingRelationship { .. }));
    }
}
