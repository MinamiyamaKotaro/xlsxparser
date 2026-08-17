//! Parses `xl/workbook.xml`'s `<sheets>` list into an ordered
//! `Vec<WorkbookSheetEntry>`, preserving source definition order.

use crate::error::Error;
use crate::model::SheetVisibility;
use crate::parse::{create_secure_reader, optional_attr, read_event, required_attr};
use quick_xml::events::Event;
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

/// Parses `xl/workbook.xml` and returns the `<sheet>` elements under
/// `<sheets>` in source order. Returns `Error::MissingRequiredElement` if
/// the `<sheets>` element itself is absent. Returns an empty `Vec` if
/// `<sheets></sheets>` is empty (a zero-sheet workbook is structurally
/// valid).
pub(crate) fn parse_workbook_xml(
    reader: impl BufRead,
    path: &str,
) -> Result<Vec<WorkbookSheetEntry>, Error> {
    let mut xml_reader = create_secure_reader(reader);
    let mut buf = Vec::new();
    let mut entries = Vec::new();
    let mut in_sheets = false;
    let mut saw_sheets = false;

    loop {
        let event = read_event(&mut xml_reader, &mut buf, path)?;
        match &event {
            Event::Start(e) if e.local_name().as_ref() == b"sheets" => {
                in_sheets = true;
                saw_sheets = true;
            }
            Event::Empty(e) if e.local_name().as_ref() == b"sheets" => {
                saw_sheets = true;
            }
            Event::End(e) if e.local_name().as_ref() == b"sheets" => {
                in_sheets = false;
            }
            Event::Start(e) | Event::Empty(e)
                if in_sheets && e.local_name().as_ref() == b"sheet" =>
            {
                let name = required_attr(e, path, "name")?;
                let r_id = required_attr(e, path, "r:id")?;
                let state = optional_attr(e, path, "state")?;
                entries.push(WorkbookSheetEntry {
                    name,
                    r_id,
                    visibility: parse_visibility(state.as_deref()),
                });
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    if !saw_sheets {
        return Err(Error::MissingRequiredElement {
            path: path.to_string(),
            name: "sheets",
        });
    }

    Ok(entries)
}

/// Converts the `state` attribute string into `SheetVisibility`. Defaults to
/// `Visible` when the attribute is absent. An unrecognized value (from a
/// future spec extension or a corrupted file) falls back to `Visible`
/// rather than erroring, since visibility is only a display hint and does
/// not affect data integrity.
fn parse_visibility(state: Option<&str>) -> SheetVisibility {
    match state {
        None | Some("visible") => SheetVisibility::Visible,
        Some("hidden") => SheetVisibility::Hidden,
        Some("veryHidden") => SheetVisibility::VeryHidden,
        Some(_) => SheetVisibility::Visible,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_sheets_preserving_order() {
        let xml = br#"<?xml version="1.0"?>
<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
    <sheet name="Sheet1" sheetId="1" r:id="rId1"/>
    <sheet name="Sheet2" sheetId="2" r:id="rId2" state="hidden"/>
  </sheets>
</workbook>"#;

        let entries = parse_workbook_xml(&xml[..], "xl/workbook.xml").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "Sheet1");
        assert_eq!(entries[0].r_id, "rId1");
        assert_eq!(entries[0].visibility, SheetVisibility::Visible);
        assert_eq!(entries[1].name, "Sheet2");
        assert_eq!(entries[1].visibility, SheetVisibility::Hidden);
    }

    #[test]
    fn absent_state_is_visible() {
        assert_eq!(parse_visibility(None), SheetVisibility::Visible);
    }

    #[test]
    fn state_hidden_and_very_hidden() {
        assert_eq!(parse_visibility(Some("hidden")), SheetVisibility::Hidden);
        assert_eq!(
            parse_visibility(Some("veryHidden")),
            SheetVisibility::VeryHidden
        );
    }

    #[test]
    fn unrecognized_state_falls_back_to_visible() {
        assert_eq!(
            parse_visibility(Some("somethingFuture")),
            SheetVisibility::Visible
        );
    }

    #[test]
    fn sheet_missing_name_is_an_error() {
        let xml = br#"<workbook><sheets><sheet r:id="rId1"/></sheets></workbook>"#;
        let err = parse_workbook_xml(&xml[..], "xl/workbook.xml").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement { name: "name", .. }
        ));
    }

    #[test]
    fn sheet_missing_r_id_is_an_error() {
        let xml = br#"<workbook><sheets><sheet name="Sheet1"/></sheets></workbook>"#;
        let err = parse_workbook_xml(&xml[..], "xl/workbook.xml").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement { name: "r:id", .. }
        ));
    }

    #[test]
    fn missing_sheets_element_is_an_error() {
        let xml = b"<workbook></workbook>";
        let err = parse_workbook_xml(&xml[..], "xl/workbook.xml").unwrap_err();
        assert!(matches!(
            err,
            Error::MissingRequiredElement { name: "sheets", .. }
        ));
    }

    #[test]
    fn empty_sheets_element_produces_empty_vec() {
        let xml = b"<workbook><sheets></sheets></workbook>";
        let entries = parse_workbook_xml(&xml[..], "xl/workbook.xml").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn self_closing_empty_sheets_element_produces_empty_vec() {
        let xml = b"<workbook><sheets/></workbook>";
        let entries = parse_workbook_xml(&xml[..], "xl/workbook.xml").unwrap();
        assert!(entries.is_empty());
    }
}
