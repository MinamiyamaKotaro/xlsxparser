// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Phase 3.5: parses `xl/drawings/drawingN.xml` — the `xdr:twoCellAnchor`/
//! `xdr:oneCellAnchor` elements that anchor a `<xdr:pic>` (embedded image)
//! to a cell position (Issue #65). Pure XML parsing only, matching this
//! module's `parse/` sibling `relationships.rs`'s division of labor:
//! `r:embed`/`r:id` (hyperlink) attributes are captured here as raw
//! relationship IDs, and left for `pipeline.rs` to resolve against
//! `drawingN.xml.rels`.

use crate::error::Error;
use crate::model::{AnchorMarker, CellRef, ImageAnchor, ImageExtent};
use crate::parse::{
    create_secure_reader, optional_attr, read_event, read_leaf_text, required_attr,
};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::BufRead;

/// One `<xdr:pic>` inside a `twoCellAnchor`/`oneCellAnchor`, before its
/// relationship IDs have been resolved to actual target paths. `pipeline.rs`
/// resolves `embed_r_id`/`hyperlink_r_id` against `drawingN.xml.rels` and
/// turns this into `model::Image`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PendingImage {
    pub anchor: ImageAnchor,
    pub embed_r_id: String,
    pub hyperlink_r_id: Option<String>,
}

/// Phase 3.5's entry function: parses one `drawingN.xml` into every
/// `<xdr:pic>` it anchors. An anchor with no `<xdr:pic>` inside it (a plain
/// shape or chart) is silently skipped — only picture anchors are this
/// library's concern (Issue #65's stated scope).
pub(crate) fn parse_drawing(reader: impl BufRead, path: &str) -> Result<Vec<PendingImage>, Error> {
    let mut xml_reader = create_secure_reader(reader);
    let mut buf = Vec::new();
    let mut images = Vec::new();

    loop {
        match read_event(&mut xml_reader, &mut buf, path)? {
            Event::Start(e) if e.local_name().as_ref() == b"twoCellAnchor" => {
                if let Some(image) = parse_anchor_body(&mut xml_reader, path, AnchorKind::TwoCell)?
                {
                    images.push(image);
                }
            }
            Event::Start(e) if e.local_name().as_ref() == b"oneCellAnchor" => {
                if let Some(image) = parse_anchor_body(&mut xml_reader, path, AnchorKind::OneCell)?
                {
                    images.push(image);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(images)
}

enum AnchorKind {
    TwoCell,
    OneCell,
}

/// Parses everything between a `<xdr:twoCellAnchor>`/`<xdr:oneCellAnchor>`'s
/// start tag and its matching end tag: `<xdr:from>`, `<xdr:to>` (`TwoCell`
/// only), `<xdr:ext>` (`OneCell` only), and — if present — a `<xdr:pic>`'s
/// `r:embed`/hyperlink `r:id`. `twoCellAnchor`/`oneCellAnchor` never nest
/// inside themselves in DrawingML, so a plain scan for the matching end tag
/// (no depth counter) is sufficient. Returns `None` if no `<xdr:pic>` is
/// found (a non-picture anchor), `Some` otherwise.
fn parse_anchor_body(
    reader: &mut Reader<impl BufRead>,
    path: &str,
    kind: AnchorKind,
) -> Result<Option<PendingImage>, Error> {
    let mut buf = Vec::new();
    let mut from: Option<AnchorMarker> = None;
    let mut to: Option<AnchorMarker> = None;
    let mut ext: Option<ImageExtent> = None;
    let mut embed_r_id: Option<String> = None;
    let mut hyperlink_r_id: Option<String> = None;
    // Tracks whether the cursor is currently inside the anchor's <xdr:pic>
    // (never self-nesting, so a plain bool suffices — unlike
    // twoCellAnchor/oneCellAnchor or grpSp). Needed because <xdr:pic>'s own
    // <xdr:spPr><a:xfrm><a:ext .../></a:xfrm></xdr:spPr> — real writers
    // emit this even for a plain, non-grouped picture — shares the same
    // local name "ext" as the OneCell anchor's own size-defining
    // <xdr:ext>. Without this guard, a `oneCellAnchor`'s <xdr:pic> (which
    // is read *after* <xdr:ext> in document order) would silently
    // overwrite the anchor's declared size with the shape's internal one —
    // a real discrepancy for a diff-oriented tool, since <xdr:ext> is the
    // size actually displayed on the sheet (Issue #65 follow-up).
    let mut in_pic = false;

    loop {
        match read_event(reader, &mut buf, path)? {
            Event::Start(e) if e.local_name().as_ref() == b"from" => {
                from = Some(parse_marker(reader, path)?);
            }
            Event::Start(e) if e.local_name().as_ref() == b"to" => {
                to = Some(parse_marker(reader, path)?);
            }
            Event::Start(e) if e.local_name().as_ref() == b"pic" => {
                in_pic = true;
            }
            Event::Empty(e) if e.local_name().as_ref() == b"ext" && !in_pic => {
                let cx = parse_attr_i64(&e, path, "cx")?;
                let cy = parse_attr_i64(&e, path, "cy")?;
                ext = Some(ImageExtent { cx, cy });
            }
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"blip" => {
                embed_r_id = Some(required_attr(&e, path, "r:embed")?);
            }
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"hlinkClick" => {
                hyperlink_r_id = optional_attr(&e, path, "r:id")?;
            }
            Event::End(e)
                if matches!(e.local_name().as_ref(), b"twoCellAnchor" | b"oneCellAnchor") =>
            {
                break;
            }
            Event::Eof => {
                return Err(Error::MissingRequiredElement {
                    path: path.to_string(),
                    name: "closing tag",
                })
            }
            _ => {}
        }
        buf.clear();
    }

    // No <xdr:pic> found (a plain shape or chart anchor) — not this
    // library's concern.
    let Some(embed_r_id) = embed_r_id else {
        return Ok(None);
    };

    let anchor = match kind {
        AnchorKind::TwoCell => ImageAnchor::TwoCell {
            from: from.ok_or_else(|| Error::MissingRequiredElement {
                path: path.to_string(),
                name: "xdr:from",
            })?,
            to: to.ok_or_else(|| Error::MissingRequiredElement {
                path: path.to_string(),
                name: "xdr:to",
            })?,
        },
        AnchorKind::OneCell => ImageAnchor::OneCell {
            from: from.ok_or_else(|| Error::MissingRequiredElement {
                path: path.to_string(),
                name: "xdr:from",
            })?,
            ext: ext.ok_or_else(|| Error::MissingRequiredElement {
                path: path.to_string(),
                name: "xdr:ext",
            })?,
        },
    };

    Ok(Some(PendingImage {
        anchor,
        embed_r_id,
        hyperlink_r_id,
    }))
}

/// Parses a `<xdr:from>`/`<xdr:to>` marker's four child leaf elements
/// (`col`/`colOff`/`row`/`rowOff`) into an `AnchorMarker`.
fn parse_marker(reader: &mut Reader<impl BufRead>, path: &str) -> Result<AnchorMarker, Error> {
    let mut buf = Vec::new();
    let mut col: Option<u32> = None;
    let mut col_off: Option<i64> = None;
    let mut row: Option<u32> = None;
    let mut row_off: Option<i64> = None;

    loop {
        match read_event(reader, &mut buf, path)? {
            Event::Start(e) if e.local_name().as_ref() == b"col" => {
                col = Some(parse_leaf(reader, path, "xdr:col")?);
            }
            Event::Start(e) if e.local_name().as_ref() == b"colOff" => {
                col_off = Some(parse_leaf(reader, path, "xdr:colOff")?);
            }
            Event::Start(e) if e.local_name().as_ref() == b"row" => {
                row = Some(parse_leaf(reader, path, "xdr:row")?);
            }
            Event::Start(e) if e.local_name().as_ref() == b"rowOff" => {
                row_off = Some(parse_leaf(reader, path, "xdr:rowOff")?);
            }
            Event::End(e) if matches!(e.local_name().as_ref(), b"from" | b"to") => break,
            Event::Eof => {
                return Err(Error::MissingRequiredElement {
                    path: path.to_string(),
                    name: "closing tag",
                })
            }
            _ => {}
        }
        buf.clear();
    }

    let col = col.ok_or_else(|| Error::MissingRequiredElement {
        path: path.to_string(),
        name: "xdr:col",
    })?;
    let row = row.ok_or_else(|| Error::MissingRequiredElement {
        path: path.to_string(),
        name: "xdr:row",
    })?;

    Ok(AnchorMarker {
        cell: zero_based_to_cell_ref(row, col, path)?,
        col_off: col_off.unwrap_or(0),
        row_off: row_off.unwrap_or(0),
    })
}

/// `xdr:col`/`xdr:row` are 0-based (unlike `CellRef`, which is 1-based to
/// match A1 notation — see `model::cell::CellRef`'s doc), so 1 is added to
/// each before constructing the `CellRef`. A value that would overflow or
/// push the result past `CellRef::MAX_ROW`/`MAX_COL` is rejected the same
/// way `CellRef::from_a1` rejects an out-of-range A1 reference (security
/// review `docs/security/code-review.md` Finding 2's rationale applies here
/// too — an attacker-controlled coordinate should never reach the model).
fn zero_based_to_cell_ref(row0: u32, col0: u32, path: &str) -> Result<CellRef, Error> {
    let row = row0.checked_add(1).filter(|&r| r <= CellRef::MAX_ROW);
    let col = col0.checked_add(1).filter(|&c| c <= CellRef::MAX_COL);
    match (row, col) {
        (Some(row), Some(col)) => Ok(CellRef { row, col }),
        _ => Err(Error::InvalidCellRef(format!(
            "drawing anchor row={row0} col={col0} (0-based) in {path}"
        ))),
    }
}

fn parse_leaf<T: std::str::FromStr>(
    reader: &mut Reader<impl BufRead>,
    path: &str,
    what: &str,
) -> Result<T, Error> {
    let text = read_leaf_text(reader, path)?;
    text.trim()
        .parse()
        .map_err(|_| Error::InvalidPackage(format!("invalid {what} {text:?} in {path}")))
}

fn parse_attr_i64(
    e: &quick_xml::events::BytesStart<'_>,
    path: &str,
    name: &'static str,
) -> Result<i64, Error> {
    let text = required_attr(e, path, name)?;
    text.trim()
        .parse()
        .map_err(|_| Error::InvalidPackage(format!("invalid {name} {text:?} in {path}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS: &str = r#"xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

    fn marker_xml(tag: &str, col: u32, col_off: i64, row: u32, row_off: i64) -> String {
        format!(
            "<xdr:{tag}><xdr:col>{col}</xdr:col><xdr:colOff>{col_off}</xdr:colOff><xdr:row>{row}</xdr:row><xdr:rowOff>{row_off}</xdr:rowOff></xdr:{tag}>"
        )
    }

    #[test]
    fn parses_two_cell_anchor_with_embed_and_hyperlink() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:twoCellAnchor>
    {from}
    {to}
    <xdr:pic>
      <xdr:nvPicPr>
        <xdr:cNvPr id="2" name="Picture 1">
          <a:hlinkClick r:id="rId2"/>
        </xdr:cNvPr>
        <xdr:cNvPicPr/>
      </xdr:nvPicPr>
      <xdr:blipFill>
        <a:blip r:embed="rId1"/>
      </xdr:blipFill>
    </xdr:pic>
    <xdr:clientData/>
  </xdr:twoCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 1, 10, 2, 20),
            to = marker_xml("to", 5, 0, 10, 0),
        );

        let images = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap();
        assert_eq!(images.len(), 1);
        let image = &images[0];
        assert_eq!(image.embed_r_id, "rId1");
        assert_eq!(image.hyperlink_r_id.as_deref(), Some("rId2"));
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
    fn parses_one_cell_anchor_with_ext_and_no_hyperlink() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    {from}
    <xdr:ext cx="952500" cy="952500"/>
    <xdr:pic>
      <xdr:nvPicPr>
        <xdr:cNvPr id="2" name="Picture 1"/>
        <xdr:cNvPicPr/>
      </xdr:nvPicPr>
      <xdr:blipFill>
        <a:blip r:embed="rId3"/>
      </xdr:blipFill>
    </xdr:pic>
  </xdr:oneCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 0, 0, 0, 0),
        );

        let images = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap();
        assert_eq!(images.len(), 1);
        let image = &images[0];
        assert_eq!(image.embed_r_id, "rId3");
        assert_eq!(image.hyperlink_r_id, None);
        assert_eq!(
            image.anchor,
            ImageAnchor::OneCell {
                from: AnchorMarker {
                    cell: CellRef { row: 1, col: 1 },
                    col_off: 0,
                    row_off: 0,
                },
                ext: ImageExtent {
                    cx: 952_500,
                    cy: 952_500,
                },
            }
        );
    }

    #[test]
    fn anchor_without_pic_is_skipped() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:twoCellAnchor>
    {from}
    {to}
    <xdr:sp/>
    <xdr:clientData/>
  </xdr:twoCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 0, 0, 0, 0),
            to = marker_xml("to", 1, 0, 1, 0),
        );

        let images = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap();
        assert!(images.is_empty());
    }

    #[test]
    fn multiple_anchors_all_parsed() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    {from1}
    <xdr:ext cx="100" cy="100"/>
    <xdr:pic><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill></xdr:pic>
  </xdr:oneCellAnchor>
  <xdr:twoCellAnchor>
    {from2}
    {to2}
    <xdr:pic><xdr:blipFill><a:blip r:embed="rId2"/></xdr:blipFill></xdr:pic>
  </xdr:twoCellAnchor>
</xdr:wsDr>"#,
            from1 = marker_xml("from", 0, 0, 0, 0),
            from2 = marker_xml("from", 2, 0, 2, 0),
            to2 = marker_xml("to", 3, 0, 3, 0),
        );

        let images = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].embed_r_id, "rId1");
        assert_eq!(images[1].embed_r_id, "rId2");
    }

    #[test]
    fn missing_blip_embed_is_missing_required_element() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    {from}
    <xdr:ext cx="100" cy="100"/>
    <xdr:pic><xdr:blipFill><a:blip/></xdr:blipFill></xdr:pic>
  </xdr:oneCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 0, 0, 0, 0),
        );

        let err = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement {
                name: "r:embed",
                ..
            }
        ));
    }

    #[test]
    fn out_of_range_row_is_invalid_cell_ref() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    {from}
    <xdr:ext cx="100" cy="100"/>
    <xdr:pic><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill></xdr:pic>
  </xdr:oneCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 0, 0, 4_294_967_295, 0),
        );

        let err = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap_err();
        assert!(matches!(err, Error::InvalidCellRef(_)));
    }

    #[test]
    fn malformed_ext_attribute_is_invalid_package() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    {from}
    <xdr:ext cx="not-a-number" cy="100"/>
    <xdr:pic><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill></xdr:pic>
  </xdr:oneCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 0, 0, 0, 0),
        );

        let err = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap_err();
        assert!(matches!(err, Error::InvalidPackage(_)));
    }

    #[test]
    fn empty_drawing_produces_no_images() {
        let xml = format!(r#"<xdr:wsDr {NS}></xdr:wsDr>"#);
        let images = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap();
        assert!(images.is_empty());
    }

    #[test]
    fn one_cell_anchor_ext_is_not_overwritten_by_pics_own_sppr_xfrm_ext() {
        // Real writers (confirmed against actual LibreOffice output) emit
        // a <xdr:pic><xdr:spPr><a:xfrm><a:ext .../></a:xfrm></xdr:spPr>
        // even for a plain, non-grouped picture — sharing the local name
        // "ext" with the OneCell anchor's own size-defining <xdr:ext>,
        // which appears earlier in document order. The anchor's declared
        // size (100, 200) must win over the shape's internal one
        // (999, 888), since it's what's actually displayed on the sheet
        // (Issue #65 follow-up).
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    {from}
    <xdr:ext cx="100" cy="200"/>
    <xdr:pic>
      <xdr:spPr>
        <a:xfrm><a:off x="1" y="2"/><a:ext cx="999" cy="888"/></a:xfrm>
      </xdr:spPr>
      <xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill>
    </xdr:pic>
  </xdr:oneCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 0, 0, 0, 0),
        );

        let images = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap();
        assert_eq!(images.len(), 1);
        match images[0].anchor {
            ImageAnchor::OneCell { ext, .. } => {
                assert_eq!(ext, ImageExtent { cx: 100, cy: 200 });
            }
            ImageAnchor::TwoCell { .. } => panic!("expected a OneCell anchor"),
        }
    }

    #[test]
    fn anchor_body_eof_before_closing_tag_is_missing_required_element() {
        let xml = format!(
            r#"<xdr:wsDr {NS}><xdr:twoCellAnchor>{from}<xdr:pic><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill></xdr:pic>"#,
            from = marker_xml("from", 0, 0, 0, 0),
        );
        let err = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement {
                name: "closing tag",
                ..
            }
        ));
    }

    #[test]
    fn two_cell_anchor_with_pic_but_no_from_is_missing_required_element() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:twoCellAnchor>
    {to}
    <xdr:pic><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill></xdr:pic>
  </xdr:twoCellAnchor>
</xdr:wsDr>"#,
            to = marker_xml("to", 1, 0, 1, 0),
        );
        let err = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement {
                name: "xdr:from",
                ..
            }
        ));
    }

    #[test]
    fn two_cell_anchor_with_pic_but_no_to_is_missing_required_element() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:twoCellAnchor>
    {from}
    <xdr:pic><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill></xdr:pic>
  </xdr:twoCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 0, 0, 0, 0),
        );
        let err = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement { name: "xdr:to", .. }
        ));
    }

    #[test]
    fn one_cell_anchor_with_pic_but_no_from_is_missing_required_element() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    <xdr:ext cx="100" cy="100"/>
    <xdr:pic><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill></xdr:pic>
  </xdr:oneCellAnchor>
</xdr:wsDr>"#
        );
        let err = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement {
                name: "xdr:from",
                ..
            }
        ));
    }

    #[test]
    fn one_cell_anchor_with_pic_but_no_ext_is_missing_required_element() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    {from}
    <xdr:pic><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill></xdr:pic>
  </xdr:oneCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 0, 0, 0, 0),
        );
        let err = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement {
                name: "xdr:ext",
                ..
            }
        ));
    }

    #[test]
    fn marker_eof_before_closing_tag_is_missing_required_element() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    <xdr:from><xdr:col>0</xdr:col><xdr:row>0</xdr:row>"#
        );
        let err = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement {
                name: "closing tag",
                ..
            }
        ));
    }

    #[test]
    fn marker_missing_col_is_missing_required_element() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    <xdr:from><xdr:row>0</xdr:row></xdr:from>
    <xdr:ext cx="100" cy="100"/>
    <xdr:pic><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill></xdr:pic>
  </xdr:oneCellAnchor>
</xdr:wsDr>"#
        );
        let err = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement {
                name: "xdr:col",
                ..
            }
        ));
    }

    #[test]
    fn marker_ignores_whitespace_between_child_elements() {
        // Pretty-printed XML puts insignificant whitespace (Event::Text)
        // between <xdr:from>'s children — parse_marker's catch-all arm must
        // simply skip it rather than mistaking it for one of col/colOff/
        // row/rowOff.
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    <xdr:from>
      <xdr:col>2</xdr:col>
      <xdr:colOff>5000</xdr:colOff>
      <xdr:row>4</xdr:row>
      <xdr:rowOff>5000</xdr:rowOff>
    </xdr:from>
    <xdr:ext cx="100" cy="100"/>
    <xdr:pic><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill></xdr:pic>
  </xdr:oneCellAnchor>
</xdr:wsDr>"#
        );
        let images = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap();
        assert_eq!(images.len(), 1);
        match images[0].anchor {
            ImageAnchor::OneCell { from, .. } => {
                assert_eq!(from.cell, CellRef { row: 5, col: 3 });
            }
            ImageAnchor::TwoCell { .. } => panic!("expected a OneCell anchor"),
        }
    }

    #[test]
    fn marker_missing_row_is_missing_required_element() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    <xdr:from><xdr:col>0</xdr:col></xdr:from>
    <xdr:ext cx="100" cy="100"/>
    <xdr:pic><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill></xdr:pic>
  </xdr:oneCellAnchor>
</xdr:wsDr>"#
        );
        let err = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement {
                name: "xdr:row",
                ..
            }
        ));
    }
}
