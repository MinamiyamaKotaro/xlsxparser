// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Phase 3.5: parses `xl/drawings/drawingN.xml` — the `xdr:twoCellAnchor`/
//! `xdr:oneCellAnchor` elements that anchor a `<xdr:pic>` (embedded image)
//! to a cell position (Issue #65), including pictures nested inside one or
//! more `<xdr:grpSp>` group shapes (Issue #67). Pure XML parsing only,
//! matching this module's `parse/` sibling `relationships.rs`'s division
//! of labor: `r:embed`/`r:id` (hyperlink) attributes are captured here as
//! raw relationship IDs, and left for `pipeline.rs` to resolve against
//! `drawingN.xml.rels`.

use crate::error::Error;
use crate::model::{AnchorMarker, CellRef, ImageAnchor, ImageExtent};
use crate::parse::{
    create_secure_reader, optional_attr, read_event, read_leaf_text, required_attr,
};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::BufRead;

/// One `<xdr:pic>` inside a `twoCellAnchor`/`oneCellAnchor` (directly, or
/// nested inside one or more `<xdr:grpSp>`), before its relationship IDs
/// have been resolved to actual target paths. `pipeline.rs` resolves
/// `embed_r_id`/`hyperlink_r_id` against `drawingN.xml.rels` and turns
/// this into `model::Image`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PendingImage {
    pub anchor: ImageAnchor,
    pub embed_r_id: String,
    pub hyperlink_r_id: Option<String>,
}

/// Phase 3.5's entry function: parses one `drawingN.xml` into every
/// `<xdr:pic>` it anchors, however deeply nested inside `<xdr:grpSp>`
/// elements. An anchor (or group) with no `<xdr:pic>` inside it at all (a
/// plain shape or chart) contributes no images — only picture anchors are
/// this library's concern (Issue #65's stated scope).
pub(crate) fn parse_drawing(reader: impl BufRead, path: &str) -> Result<Vec<PendingImage>, Error> {
    let mut xml_reader = create_secure_reader(reader);
    let mut buf = Vec::new();
    let mut images = Vec::new();

    loop {
        match read_event(&mut xml_reader, &mut buf, path)? {
            Event::Start(e) if e.local_name().as_ref() == b"twoCellAnchor" => {
                images.extend(parse_anchor_body(
                    &mut xml_reader,
                    path,
                    AnchorKind::TwoCell,
                )?);
            }
            Event::Start(e) if e.local_name().as_ref() == b"oneCellAnchor" => {
                images.extend(parse_anchor_body(
                    &mut xml_reader,
                    path,
                    AnchorKind::OneCell,
                )?);
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

/// Defensive cap on `<xdr:grpSp>` nesting depth (security review Finding 1,
/// Issue #71 follow-up). `resolve_grouped_pic` costs O(current nesting
/// depth) per `<xdr:pic>` resolved, so a drawing part with `D` levels of
/// nesting and `N` sibling pictures at the innermost level costs O(N * D)
/// while costing only O(N + D) bytes to construct — the Zip Bomb byte-size
/// cap alone does not bound `D` (the same shape as `resolve::merge`'s
/// `MAX_MERGE_REGIONS`, which bounds merge-cell count for the same reason).
/// Real-world LibreOffice/Excel-authored group nesting is essentially never
/// more than a handful of levels deep, so 64 leaves enormous headroom over
/// any legitimate file while capping worst-case cost at O(N * 64).
pub(crate) const MAX_GROUP_NESTING_DEPTH: usize = 64;

/// A `<xdr:grpSp>`'s own `<xdr:grpSpPr><a:xfrm>` — `off`/`ext` describe the
/// group's bounding box (in its *parent*'s coordinate space: the anchor's
/// own space for the outermost group, or the enclosing group's child space
/// for a nested one), `chOff`/`chExt` describe the coordinate space its own
/// children's `off`/`ext` are expressed in. See `resolve_grouped_pic`.
#[derive(Debug, Clone, Copy, Default)]
struct GroupContext {
    off_x: i64,
    off_y: i64,
    ext_cx: i64,
    ext_cy: i64,
    ch_off_x: i64,
    ch_off_y: i64,
    ch_ext_cx: i64,
    ch_ext_cy: i64,
}

/// Parses everything between a `<xdr:twoCellAnchor>`/`<xdr:oneCellAnchor>`'s
/// start tag and its matching end tag: `<xdr:from>`, `<xdr:to>` (`TwoCell`
/// only), `<xdr:ext>` (`OneCell` only), and every `<xdr:pic>` found —
/// whether directly under the anchor or nested inside one or more
/// `<xdr:grpSp>` — each contributing one `PendingImage`. An anchor whose
/// sole child is a non-picture shape (`<xdr:sp>`, a chart, ...) or an empty
/// group contributes none.
///
/// `group_stack` doubles as the nesting-depth tracker for `<xdr:grpSp>`
/// (which, unlike `twoCellAnchor`/`oneCellAnchor`, *can* nest inside
/// itself): pushed on `<xdr:grpSp>`'s start, popped on its end, so the
/// stack's own length is always the current nesting depth — no separate
/// counter is needed.
fn parse_anchor_body(
    reader: &mut Reader<impl BufRead>,
    path: &str,
    kind: AnchorKind,
) -> Result<Vec<PendingImage>, Error> {
    let mut buf = Vec::new();
    let mut from: Option<AnchorMarker> = None;
    let mut to: Option<AnchorMarker> = None;
    let mut ext: Option<ImageExtent> = None;
    let mut images = Vec::new();
    let mut group_stack: Vec<GroupContext> = Vec::new();

    // State for the <xdr:pic> currently being read, and for the
    // <xdr:grpSpPr> currently being read (if any) — both reset once their
    // owning element closes, so nothing leaks into a sibling.
    let mut in_pic = false;
    let mut in_sp_pr = false;
    let mut in_grp_sp_pr = false;
    let mut embed_r_id: Option<String> = None;
    let mut hyperlink_r_id: Option<String> = None;
    let mut pic_off = (0i64, 0i64);
    let mut pic_ext = (0i64, 0i64);

    loop {
        let event = read_event(reader, &mut buf, path)?;
        match event {
            Event::Start(e) => match e.local_name().as_ref() {
                b"from" => {
                    from = Some(parse_marker(reader, path)?);
                }
                b"to" => {
                    to = Some(parse_marker(reader, path)?);
                }
                b"pic" => {
                    in_pic = true;
                }
                b"spPr" if in_pic && !group_stack.is_empty() => {
                    in_sp_pr = true;
                }
                b"blip" => {
                    embed_r_id = Some(required_attr(&e, path, "r:embed")?);
                }
                b"hlinkClick" if in_pic => {
                    hyperlink_r_id = optional_attr(&e, path, "r:id")?;
                }
                b"grpSp" => {
                    if group_stack.len() >= MAX_GROUP_NESTING_DEPTH {
                        return Err(Error::TooManyNestedGroups {
                            path: path.to_string(),
                            depth: group_stack.len() + 1,
                            limit: MAX_GROUP_NESTING_DEPTH,
                        });
                    }
                    group_stack.push(GroupContext::default());
                }
                b"grpSpPr" => {
                    in_grp_sp_pr = true;
                }
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                b"pic" => {
                    if let Some(id) = embed_r_id.take() {
                        let anchor = if group_stack.is_empty() {
                            match kind {
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
                            }
                        } else {
                            // A grouped picture always resolves to a OneCell
                            // shape (explicit cell + offset + size) regardless
                            // of the enclosing anchor's own kind — its
                            // position/size come entirely from the group
                            // transform, not from a `to` marker of its own.
                            let base = from.ok_or_else(|| Error::MissingRequiredElement {
                                path: path.to_string(),
                                name: "xdr:from",
                            })?;
                            let (dx, dy, cx, cy) =
                                resolve_grouped_pic(&group_stack, path, pic_off, pic_ext)?;
                            ImageAnchor::OneCell {
                                from: AnchorMarker {
                                    cell: base.cell,
                                    col_off: base.col_off + dx,
                                    row_off: base.row_off + dy,
                                },
                                ext: ImageExtent { cx, cy },
                            }
                        };
                        images.push(PendingImage {
                            anchor,
                            embed_r_id: id,
                            hyperlink_r_id: hyperlink_r_id.take(),
                        });
                    }
                    in_pic = false;
                    pic_off = (0, 0);
                    pic_ext = (0, 0);
                }
                b"spPr" => {
                    in_sp_pr = false;
                }
                b"grpSp" => {
                    group_stack.pop();
                }
                b"grpSpPr" => {
                    in_grp_sp_pr = false;
                }
                b"twoCellAnchor" | b"oneCellAnchor" if group_stack.is_empty() => {
                    break;
                }
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                b"off" => {
                    let x = parse_attr_i64(&e, path, "x")?;
                    let y = parse_attr_i64(&e, path, "y")?;
                    if in_grp_sp_pr {
                        if let Some(g) = group_stack.last_mut() {
                            g.off_x = x;
                            g.off_y = y;
                        }
                    } else if in_sp_pr {
                        pic_off = (x, y);
                    }
                }
                b"ext" => {
                    if in_grp_sp_pr {
                        let cx = parse_attr_i64(&e, path, "cx")?;
                        let cy = parse_attr_i64(&e, path, "cy")?;
                        if let Some(g) = group_stack.last_mut() {
                            g.ext_cx = cx;
                            g.ext_cy = cy;
                        }
                    } else if in_sp_pr {
                        pic_ext = (
                            parse_attr_i64(&e, path, "cx")?,
                            parse_attr_i64(&e, path, "cy")?,
                        );
                    } else if !in_pic {
                        // The anchor's own OneCell size — see Issue #65's
                        // follow-up doc note for why this dispatch (anchor-level
                        // / group-level / pic-level) exists. `!in_pic` also
                        // covers a *non-grouped* pic's own spPr/xfrm/ext (whose
                        // spPr is never tracked via `in_sp_pr` at all — see the
                        // `spPr` match arm's `!group_stack.is_empty()` guard —
                        // so without this it would otherwise fall through here
                        // and silently overwrite the anchor's own value).
                        let cx = parse_attr_i64(&e, path, "cx")?;
                        let cy = parse_attr_i64(&e, path, "cy")?;
                        ext = Some(ImageExtent { cx, cy });
                    }
                }
                b"chOff" if in_grp_sp_pr => {
                    let x = parse_attr_i64(&e, path, "x")?;
                    let y = parse_attr_i64(&e, path, "y")?;
                    if let Some(g) = group_stack.last_mut() {
                        g.ch_off_x = x;
                        g.ch_off_y = y;
                    }
                }
                b"chExt" if in_grp_sp_pr => {
                    let cx = parse_attr_i64(&e, path, "cx")?;
                    let cy = parse_attr_i64(&e, path, "cy")?;
                    if let Some(g) = group_stack.last_mut() {
                        g.ch_ext_cx = cx;
                        g.ch_ext_cy = cy;
                    }
                }
                b"blip" => {
                    embed_r_id = Some(required_attr(&e, path, "r:embed")?);
                }
                b"hlinkClick" if in_pic => {
                    hyperlink_r_id = optional_attr(&e, path, "r:id")?;
                }
                _ => {}
            },
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

    Ok(images)
}

/// Resolves a `<xdr:pic>` nested inside one or more `<xdr:grpSp>` levels
/// (`group_stack`, outermost first — the order groups are pushed onto the
/// stack while parsing) into `(col_off_delta, row_off_delta, cx, cy)`: the
/// EMU delta to add onto the anchor's own `from` marker, and the picture's
/// final size. Applies each level's linear transform
/// (`child' = off + (child - chOff) * (ext/chExt)`) from innermost to
/// outermost — `group_stack` is iterated in reverse for this reason — with
/// one exception: the *outermost* group's own `off` is excluded (treated
/// as 0), since it is taken to coincide with the anchor's `from` point.
///
/// That assumption was verified against real LibreOffice-generated output
/// (Issue #67 design discussion): the outermost group's `off`/`ext` are
/// literal absolute-canvas EMU coordinates, not a value relative to `from`
/// — but since `from`'s own true absolute position is, by construction,
/// the exact same physical point the group's `off` describes (any writer
/// must maintain this or the file wouldn't render correctly), the two
/// values cancel out when computing a *delta*. No row-height or
/// column-width lookup is ever needed.
fn resolve_grouped_pic(
    group_stack: &[GroupContext],
    path: &str,
    pic_off: (i64, i64),
    pic_ext: (i64, i64),
) -> Result<(i64, i64, i64, i64), Error> {
    let (mut x, mut y) = (pic_off.0 as f64, pic_off.1 as f64);
    let (mut cx, mut cy) = (pic_ext.0 as f64, pic_ext.1 as f64);

    for (i, g) in group_stack.iter().enumerate().rev() {
        if g.ch_ext_cx == 0 || g.ch_ext_cy == 0 {
            return Err(Error::InvalidPackage(format!(
                "drawing group chExt has zero dimension in {path}"
            )));
        }
        let scale_x = g.ext_cx as f64 / g.ch_ext_cx as f64;
        let scale_y = g.ext_cy as f64 / g.ch_ext_cy as f64;
        cx *= scale_x;
        cy *= scale_y;
        let off_x = if i == 0 { 0.0 } else { g.off_x as f64 };
        let off_y = if i == 0 { 0.0 } else { g.off_y as f64 };
        x = off_x + (x - g.ch_off_x as f64) * scale_x;
        y = off_y + (y - g.ch_off_y as f64) * scale_y;
    }

    // Defensive plausibility bound (security review Finding 2, Issue #71
    // follow-up): each group level's off/ext are individually
    // attacker-controlled i64 values with no bound of their own, and their
    // scale factors compound multiplicatively across nesting levels — a
    // handful of levels with an extreme ext/chExt ratio can drive x/y/cx/cy
    // to f64::INFINITY (silently saturating to i64::MAX below, via Rust's
    // saturating float-to-int cast) or merely to an absurd-but-finite
    // magnitude, either way propagating unchecked into the public
    // AnchorMarker/ImageExtent API. Rejecting outright (rather than
    // clamping to the bound) matches this function's existing `chExt == 0`
    // policy: a well-formed-but-nonsensical numeric value is treated as a
    // broken package, not silently coerced.
    if !x.is_finite() || !y.is_finite() || !cx.is_finite() || !cy.is_finite() {
        return Err(Error::InvalidPackage(format!(
            "drawing group transform produced a non-finite coordinate in {path}"
        )));
    }
    if x.abs() > MAX_PLAUSIBLE_EMU
        || y.abs() > MAX_PLAUSIBLE_EMU
        || cx.abs() > MAX_PLAUSIBLE_EMU
        || cy.abs() > MAX_PLAUSIBLE_EMU
    {
        return Err(Error::InvalidPackage(format!(
            "drawing group transform produced an implausibly large coordinate in {path}"
        )));
    }

    Ok((
        x.round() as i64,
        y.round() as i64,
        cx.round() as i64,
        cy.round() as i64,
    ))
}

/// Defensive plausibility ceiling for a resolved EMU coordinate/extent
/// (security review Finding 2, Issue #71 follow-up) — 10^12 EMU is
/// approximately 27.7 km (914,400 EMU per inch), several orders of
/// magnitude beyond Excel's real maximum sheet extent (1,048,576 rows *
/// default row height is on the order of 2*10^11 EMU), so this never
/// rejects a legitimate file while still catching a crafted group-nesting
/// chain designed to overflow toward `i64::MAX`.
const MAX_PLAUSIBLE_EMU: f64 = 1_000_000_000_000.0;

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
            Event::Start(e) => match e.local_name().as_ref() {
                b"col" => {
                    col = Some(parse_leaf(reader, path, "xdr:col")?);
                }
                b"colOff" => {
                    col_off = Some(parse_leaf(reader, path, "xdr:colOff")?);
                }
                b"row" => {
                    row = Some(parse_leaf(reader, path, "xdr:row")?);
                }
                b"rowOff" => {
                    row_off = Some(parse_leaf(reader, path, "xdr:rowOff")?);
                }
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                b"from" | b"to" => break,
                _ => {}
            },
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

    /// Advances `reader` past a marker's own opening tag (`parse_marker`'s
    /// callers always do this via the enclosing anchor's event loop before
    /// calling it) and returns the parsed `AnchorMarker`.
    fn parse_marker_standalone(xml: &str) -> Result<AnchorMarker, Error> {
        let mut reader = create_secure_reader(xml.as_bytes());
        let mut buf = Vec::new();
        read_event(&mut reader, &mut buf, "drawing1.xml").unwrap(); // consume <xdr:from>/<xdr:to>
        buf.clear();
        parse_marker(&mut reader, "drawing1.xml")
    }

    #[test]
    fn parse_marker_converts_zero_based_col_row_to_one_based_cell_ref() {
        let marker = parse_marker_standalone(&marker_xml("from", 4, 12345, 9, 67890)).unwrap();
        assert_eq!(marker.cell, CellRef { row: 10, col: 5 });
        assert_eq!(marker.col_off, 12345);
        assert_eq!(marker.row_off, 67890);
    }

    #[test]
    fn parse_marker_defaults_missing_col_off_and_row_off_to_zero() {
        let xml = "<xdr:to><xdr:col>2</xdr:col><xdr:row>3</xdr:row></xdr:to>";
        let marker = parse_marker_standalone(xml).unwrap();
        assert_eq!(marker.cell, CellRef { row: 4, col: 3 });
        assert_eq!(marker.col_off, 0);
        assert_eq!(marker.row_off, 0);
    }

    #[test]
    fn parse_marker_ignores_an_unrecognized_non_self_closing_child() {
        // An explicit (non-self-closing) child element neither col/colOff/
        // row/rowOff nor from/to's own closing tag must be skipped by both
        // of parse_marker's nested match catch-alls (its Start arm's and
        // its End arm's), not just the outer catch-all that whitespace
        // Text events hit (see marker_ignores_whitespace_between_child_
        // elements below).
        let xml = "<xdr:from><xdr:extLst></xdr:extLst><xdr:col>2</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>3</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from>";
        let marker = parse_marker_standalone(xml).unwrap();
        assert_eq!(marker.cell, CellRef { row: 4, col: 3 });
    }

    #[test]
    fn blip_and_hlink_click_as_explicit_start_end_tags_are_still_parsed() {
        // Real writers always self-close <a:blip>/<a:hlinkClick> (they
        // never have children), but both are still valid, semantically
        // identical XML written as an explicit open/close pair instead —
        // and parse_anchor_body's nested match handles that shape via a
        // separate arm (under Event::Start) from the self-closing one
        // (under Event::Empty), so both need their own coverage.
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:twoCellAnchor>
    {from}
    {to}
    <xdr:pic>
      <xdr:nvPicPr>
        <xdr:cNvPr id="2" name="Picture 1">
          <a:hlinkClick r:id="rId2"></a:hlinkClick>
        </xdr:cNvPr>
        <xdr:cNvPicPr/>
      </xdr:nvPicPr>
      <xdr:blipFill>
        <a:blip r:embed="rId1"></a:blip>
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
        assert_eq!(images[0].embed_r_id, "rId1");
        assert_eq!(images[0].hyperlink_r_id.as_deref(), Some("rId2"));
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
    fn pic_without_blip_is_skipped() {
        // A <xdr:pic> that closes without ever containing an <a:blip> (e.g.
        // a picture placeholder with no image data) has no embed_r_id to
        // take, so it must contribute zero images rather than erroring.
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:twoCellAnchor>
    {from}
    {to}
    <xdr:pic><xdr:blipFill></xdr:blipFill></xdr:pic>
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
        // (Issue #65 follow-up). Note this pic isn't inside a group, so its
        // own spPr/xfrm isn't even parsed at all (see the `in_pic &&
        // !group_stack.is_empty()` guard) — this test's `<a:ext>` inside
        // `<xdr:spPr>` never gets a chance to be misread as the group- or
        // pic-level value; it's simply skipped, and only the true
        // anchor-level `<xdr:ext>` above ever populates `ext`.
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
        assert!(matches!(
            images[0].anchor,
            ImageAnchor::OneCell {
                ext: ImageExtent { cx: 100, cy: 200 },
                ..
            }
        ));
    }

    #[test]
    fn anchor_body_eof_before_closing_tag_is_missing_required_element() {
        // The <xdr:pic> itself closes properly (and, with both from/to
        // present, finalizes into a PendingImage without error) — the EOF
        // this test targets is specifically the anchor's own missing
        // </xdr:twoCellAnchor>, reached only *after* that successful
        // per-pic finalization.
        let xml = format!(
            r#"<xdr:wsDr {NS}><xdr:twoCellAnchor>{from}{to}<xdr:pic><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill></xdr:pic>"#,
            from = marker_xml("from", 0, 0, 0, 0),
            to = marker_xml("to", 1, 0, 1, 0),
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
        assert_eq!(
            images[0].anchor,
            ImageAnchor::OneCell {
                from: AnchorMarker {
                    cell: CellRef { row: 5, col: 3 },
                    col_off: 5000,
                    row_off: 5000,
                },
                ext: ImageExtent { cx: 100, cy: 100 },
            }
        );
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

    /// Builds one `<xdr:pic>` element (real writer shape, matching what
    /// `tests/fixtures/complex.rs`'s hand-authored images already use).
    fn pic_xml(embed_r_id: &str, hyperlink: Option<&str>) -> String {
        let hlink = hyperlink
            .map(|id| format!(r#"<a:hlinkClick r:id="{id}"/>"#))
            .unwrap_or_default();
        format!(
            r#"<xdr:pic>
        <xdr:nvPicPr><xdr:cNvPr id="2" name="Picture">{hlink}</xdr:cNvPr><xdr:cNvPicPr/></xdr:nvPicPr>
        <xdr:blipFill><a:blip r:embed="{embed_r_id}"/></xdr:blipFill>
        <xdr:spPr><a:xfrm><a:off x="1080000" y="720000"/><a:ext cx="720000" cy="360000"/></a:xfrm></xdr:spPr>
      </xdr:pic>"#
        )
    }

    #[test]
    fn single_level_group_resolves_each_pic_relative_to_from() {
        // Matches the structure and numeric conventions confirmed against
        // real LibreOffice output (Issue #67): the outermost group's own
        // chOff/chExt equal its off/ext (scale 1), and both group- and
        // pic-level off/ext are literal absolute-canvas EMU. Two pics at
        // absolute (1_080_000, 720_000) and (2_160_000, 720_000) inside a
        // group whose own off equals the first pic's position.
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:twoCellAnchor editAs="absolute">
    {from}
    {to}
    <xdr:grpSp>
      <xdr:nvGrpSpPr><xdr:cNvPr id="1" name=""/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr>
      <xdr:grpSpPr>
        <a:xfrm>
          <a:off x="1080000" y="720000"/><a:ext cx="1800000" cy="540000"/>
          <a:chOff x="1080000" y="720000"/><a:chExt cx="1800000" cy="540000"/>
        </a:xfrm>
      </xdr:grpSpPr>
      {pic1}
      {pic2}
    </xdr:grpSp>
    <xdr:clientData/>
  </xdr:twoCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 1, 267_120, 4, 69_840),
            to = marker_xml("to", 3, 441_720, 7, 122_040),
            pic1 = pic_xml("rId1", None),
            pic2 = {
                // second pic at absolute (2_160_000, 720_000), size (720_000, 540_000)
                let mut s = pic_xml("rId2", Some("rId3"));
                s = s.replace(
                    r#"<a:off x="1080000" y="720000"/><a:ext cx="720000" cy="360000"/>"#,
                    r#"<a:off x="2160000" y="720000"/><a:ext cx="720000" cy="540000"/>"#,
                );
                s
            },
        );

        let images = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap();
        assert_eq!(images.len(), 2);

        assert_eq!(images[0].embed_r_id, "rId1");
        assert_eq!(images[0].hyperlink_r_id, None);
        assert_eq!(
            images[0].anchor,
            ImageAnchor::OneCell {
                from: AnchorMarker {
                    cell: CellRef { row: 5, col: 2 }, // same cell as the anchor's own `from`
                    col_off: 267_120,                 // delta 0, same as the group's own off
                    row_off: 69_840,
                },
                ext: ImageExtent {
                    cx: 720_000,
                    cy: 360_000,
                },
            }
        );

        assert_eq!(images[1].embed_r_id, "rId2");
        assert_eq!(images[1].hyperlink_r_id.as_deref(), Some("rId3"));
        // delta = 2_160_000 - 1_080_000 = 1_080_000, added to from's own colOff.
        assert_eq!(
            images[1].anchor,
            ImageAnchor::OneCell {
                from: AnchorMarker {
                    cell: CellRef { row: 5, col: 2 },
                    col_off: 267_120 + 1_080_000,
                    row_off: 69_840,
                },
                ext: ImageExtent {
                    cx: 720_000,
                    cy: 540_000
                },
            }
        );
    }

    #[test]
    fn nested_group_resolves_correctly() {
        // Mirrors the pure-math PoC's Case 3 (validated by hand-trace):
        // outer (scale 1, chOff 0), middle (off=(100_000,50_000), scale 1),
        // inner (off=(10_000,5_000), scale 0.25), pic at local
        // (40_000, 20_000) size (80_000, 40_000).
        // Expected resolved delta: (120_000, 60_000), size (20_000, 10_000).
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    {from}
    <xdr:ext cx="1" cy="1"/>
    <xdr:grpSp>
      <xdr:nvGrpSpPr><xdr:cNvPr id="1" name=""/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr>
      <xdr:grpSpPr><a:xfrm>
        <a:off x="777" y="777"/><a:ext cx="1000000" cy="1000000"/>
        <a:chOff x="0" y="0"/><a:chExt cx="1000000" cy="1000000"/>
      </a:xfrm></xdr:grpSpPr>
      <xdr:grpSp>
        <xdr:nvGrpSpPr><xdr:cNvPr id="2" name=""/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr>
        <xdr:grpSpPr><a:xfrm>
          <a:off x="100000" y="50000"/><a:ext cx="400000" cy="200000"/>
          <a:chOff x="0" y="0"/><a:chExt cx="400000" cy="200000"/>
        </a:xfrm></xdr:grpSpPr>
        <xdr:grpSp>
          <xdr:nvGrpSpPr><xdr:cNvPr id="3" name=""/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr>
          <xdr:grpSpPr><a:xfrm>
            <a:off x="10000" y="5000"/><a:ext cx="100000" cy="50000"/>
            <a:chOff x="0" y="0"/><a:chExt cx="400000" cy="200000"/>
          </a:xfrm></xdr:grpSpPr>
          <xdr:pic>
            <xdr:nvPicPr><xdr:cNvPr id="4" name=""/><xdr:cNvPicPr/></xdr:nvPicPr>
            <xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill>
            <xdr:spPr><a:xfrm><a:off x="40000" y="20000"/><a:ext cx="80000" cy="40000"/></a:xfrm></xdr:spPr>
          </xdr:pic>
        </xdr:grpSp>
      </xdr:grpSp>
    </xdr:grpSp>
  </xdr:oneCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 0, 1000, 0, 2000),
        );

        let images = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(
            images[0].anchor,
            ImageAnchor::OneCell {
                from: AnchorMarker {
                    cell: CellRef { row: 1, col: 1 },
                    col_off: 1000 + 120_000,
                    row_off: 2000 + 60_000,
                },
                ext: ImageExtent {
                    cx: 20_000,
                    cy: 10_000
                },
            }
        );
    }

    /// Builds `depth` levels of trivially-valid nested `<xdr:grpSp>` (scale
    /// 1, non-zero chExt so the existing zero-dimension guard never fires),
    /// with one `<xdr:pic>` at the innermost level.
    fn nested_groups_xml(depth: usize) -> String {
        let open: String = (0..depth)
            .map(|i| {
                format!(
                    r#"<xdr:grpSp><xdr:nvGrpSpPr><xdr:cNvPr id="{}" name=""/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr><xdr:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="100" cy="100"/><a:chOff x="0" y="0"/><a:chExt cx="100" cy="100"/></a:xfrm></xdr:grpSpPr>"#,
                    i + 1
                )
            })
            .collect();
        let close = "</xdr:grpSp>".repeat(depth);
        format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    {from}
    <xdr:ext cx="1" cy="1"/>
    {open}
    {pic}
    {close}
  </xdr:oneCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 0, 0, 0, 0),
            pic = pic_xml("rId1", None),
        )
    }

    #[test]
    fn group_nesting_depth_at_the_limit_is_accepted() {
        let xml = nested_groups_xml(MAX_GROUP_NESTING_DEPTH);
        let images = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap();
        assert_eq!(images.len(), 1);
    }

    #[test]
    fn group_nesting_depth_over_the_limit_is_too_many_nested_groups() {
        let xml = nested_groups_xml(MAX_GROUP_NESTING_DEPTH + 1);
        let err = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap_err();
        assert!(matches!(
            err,
            Error::TooManyNestedGroups {
                depth,
                limit,
                ..
            } if depth == MAX_GROUP_NESTING_DEPTH + 1 && limit == MAX_GROUP_NESTING_DEPTH
        ));
    }

    #[test]
    fn extreme_group_transform_scale_is_rejected_as_invalid_package() {
        // A handful of levels with an extreme ext/chExt ratio compounds
        // multiplicatively — this drives the resolved coordinate well past
        // MAX_PLAUSIBLE_EMU (and, with enough levels, to f64::INFINITY)
        // long before MAX_GROUP_NESTING_DEPTH would ever be reached.
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    {from}
    <xdr:ext cx="1" cy="1"/>
    <xdr:grpSp>
      <xdr:nvGrpSpPr><xdr:cNvPr id="1" name=""/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr>
      <xdr:grpSpPr><a:xfrm>
        <a:off x="0" y="0"/><a:ext cx="9223372036854775807" cy="9223372036854775807"/>
        <a:chOff x="0" y="0"/><a:chExt cx="1" cy="1"/>
      </a:xfrm></xdr:grpSpPr>
      <xdr:grpSp>
        <xdr:nvGrpSpPr><xdr:cNvPr id="2" name=""/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr>
        <xdr:grpSpPr><a:xfrm>
          <a:off x="0" y="0"/><a:ext cx="9223372036854775807" cy="9223372036854775807"/>
          <a:chOff x="0" y="0"/><a:chExt cx="1" cy="1"/>
        </a:xfrm></xdr:grpSpPr>
        {pic}
      </xdr:grpSp>
    </xdr:grpSp>
  </xdr:oneCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 0, 0, 0, 0),
            pic = pic_xml("rId1", None),
        );

        let err = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap_err();
        assert!(matches!(err, Error::InvalidPackage(_)));
    }

    #[test]
    fn group_transform_overflowing_to_infinity_is_rejected_as_invalid_package() {
        // 20 levels (well under MAX_GROUP_NESTING_DEPTH) each scaling by
        // ~9.22e18 compounds past f64::MAX (~1.8e308), producing
        // f64::INFINITY rather than merely an implausibly large finite
        // value — the `is_finite()` branch, distinct from the
        // MAX_PLAUSIBLE_EMU magnitude check the previous test exercises.
        let levels = 20;
        let open: String = (0..levels)
            .map(|i| {
                format!(
                    r#"<xdr:grpSp><xdr:nvGrpSpPr><xdr:cNvPr id="{}" name=""/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr><xdr:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="9223372036854775807" cy="9223372036854775807"/><a:chOff x="0" y="0"/><a:chExt cx="1" cy="1"/></a:xfrm></xdr:grpSpPr>"#,
                    i + 1
                )
            })
            .collect();
        let close = "</xdr:grpSp>".repeat(levels);
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    {from}
    <xdr:ext cx="1" cy="1"/>
    {open}
    {pic}
    {close}
  </xdr:oneCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 0, 0, 0, 0),
            pic = pic_xml("rId1", None),
        );

        let err = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap_err();
        assert!(matches!(err, Error::InvalidPackage(_)));
    }

    #[test]
    fn group_level_hyperlink_does_not_leak_into_first_pic() {
        // <xdr:nvGrpSpPr><xdr:cNvPr><a:hlinkClick> (a hyperlink on the
        // group itself) appears before any <xdr:pic> in document order —
        // it must not bleed into the first picture's own hyperlink_r_id.
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:twoCellAnchor editAs="absolute">
    {from}
    {to}
    <xdr:grpSp>
      <xdr:nvGrpSpPr>
        <xdr:cNvPr id="1" name="Group 1"><a:hlinkClick r:id="rIdGroupLink"/></xdr:cNvPr>
        <xdr:cNvGrpSpPr/>
      </xdr:nvGrpSpPr>
      <xdr:grpSpPr><a:xfrm>
        <a:off x="0" y="0"/><a:ext cx="100" cy="100"/>
        <a:chOff x="0" y="0"/><a:chExt cx="100" cy="100"/>
      </a:xfrm></xdr:grpSpPr>
      {pic}
    </xdr:grpSp>
  </xdr:twoCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 0, 0, 0, 0),
            to = marker_xml("to", 1, 0, 1, 0),
            pic = pic_xml("rId1", None),
        );

        let images = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].hyperlink_r_id, None);
    }

    #[test]
    fn hyperlink_does_not_leak_from_one_grouped_pic_to_the_next() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:twoCellAnchor editAs="absolute">
    {from}
    {to}
    <xdr:grpSp>
      <xdr:nvGrpSpPr><xdr:cNvPr id="1" name=""/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr>
      <xdr:grpSpPr><a:xfrm>
        <a:off x="0" y="0"/><a:ext cx="100" cy="100"/>
        <a:chOff x="0" y="0"/><a:chExt cx="100" cy="100"/>
      </a:xfrm></xdr:grpSpPr>
      {pic1}
      {pic2}
    </xdr:grpSp>
  </xdr:twoCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 0, 0, 0, 0),
            to = marker_xml("to", 1, 0, 1, 0),
            pic1 = pic_xml("rId1", Some("rIdHyperlink1")),
            pic2 = pic_xml("rId2", None),
        );

        let images = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].hyperlink_r_id.as_deref(), Some("rIdHyperlink1"));
        assert_eq!(images[1].hyperlink_r_id, None);
    }

    #[test]
    fn group_with_zero_ch_ext_is_invalid_package() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:twoCellAnchor editAs="absolute">
    {from}
    {to}
    <xdr:grpSp>
      <xdr:nvGrpSpPr><xdr:cNvPr id="1" name=""/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr>
      <xdr:grpSpPr><a:xfrm>
        <a:off x="0" y="0"/><a:ext cx="100" cy="100"/>
        <a:chOff x="0" y="0"/><a:chExt cx="0" cy="100"/>
      </a:xfrm></xdr:grpSpPr>
      {pic}
    </xdr:grpSp>
  </xdr:twoCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 0, 0, 0, 0),
            to = marker_xml("to", 1, 0, 1, 0),
            pic = pic_xml("rId1", None),
        );

        let err = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap_err();
        assert!(matches!(err, Error::InvalidPackage(_)));
    }

    #[test]
    fn group_with_only_non_picture_shapes_yields_no_images() {
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:twoCellAnchor editAs="absolute">
    {from}
    {to}
    <xdr:grpSp>
      <xdr:nvGrpSpPr><xdr:cNvPr id="1" name=""/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr>
      <xdr:grpSpPr><a:xfrm>
        <a:off x="0" y="0"/><a:ext cx="100" cy="100"/>
        <a:chOff x="0" y="0"/><a:chExt cx="100" cy="100"/>
      </a:xfrm></xdr:grpSpPr>
      <xdr:sp><xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="10" cy="10"/></a:xfrm></xdr:spPr></xdr:sp>
    </xdr:grpSp>
  </xdr:twoCellAnchor>
</xdr:wsDr>"#,
            from = marker_xml("from", 0, 0, 0, 0),
            to = marker_xml("to", 1, 0, 1, 0),
        );

        let images = parse_drawing(xml.as_bytes(), "xl/drawings/drawing1.xml").unwrap();
        assert!(images.is_empty());
    }

    #[test]
    fn grouped_pic_with_anchor_missing_from_is_missing_required_element() {
        // A grouped picture's resolved position is always anchored at the
        // enclosing anchor's own `from` marker (see `resolve_grouped_pic`'s
        // doc) — if that marker itself is missing, resolution must fail
        // the same way the non-grouped path already does, not silently
        // treat the missing base as (0, 0).
        let xml = format!(
            r#"<xdr:wsDr {NS}>
  <xdr:oneCellAnchor>
    <xdr:ext cx="1" cy="1"/>
    <xdr:grpSp>
      <xdr:nvGrpSpPr><xdr:cNvPr id="1" name=""/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr>
      <xdr:grpSpPr><a:xfrm>
        <a:off x="0" y="0"/><a:ext cx="100" cy="100"/>
        <a:chOff x="0" y="0"/><a:chExt cx="100" cy="100"/>
      </a:xfrm></xdr:grpSpPr>
      {pic}
    </xdr:grpSp>
  </xdr:oneCellAnchor>
</xdr:wsDr>"#,
            pic = pic_xml("rId1", None),
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
}
