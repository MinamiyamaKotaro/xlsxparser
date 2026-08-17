//! `Workbook`: the final resolved data model, returned by `parse_workbook`.

use crate::model::sheet::Sheet;

/// The final resolved output model. The return value of
/// `lib.rs::parse_workbook`.
#[derive(Debug, Clone)]
pub struct Workbook {
    sheets: Vec<Sheet>,
}

impl Workbook {
    /// Builds a `Workbook` from a list of already-resolved sheets.
    /// `pipeline.rs` calls this exactly once, after Phases 3 and 4 have
    /// completed for every sheet.
    pub(crate) fn new(sheets: Vec<Sheet>) -> Self {
        Self { sheets }
    }

    /// The list of sheets, preserving the definition order from the source
    /// file.
    pub fn sheets(&self) -> &[Sheet] {
        &self.sheets
    }

    /// Looks up a sheet by name.
    pub fn sheet(&self, name: &str) -> Option<&Sheet> {
        self.sheets.iter().find(|s| s.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::sheet::SheetVisibility;

    fn sheet(name: &str) -> Sheet {
        Sheet::new(name.to_string(), SheetVisibility::Visible)
    }

    #[test]
    fn sheet_lookup_by_name() {
        let workbook = Workbook::new(vec![sheet("Sheet1"), sheet("Sheet2")]);
        assert_eq!(
            workbook.sheet("Sheet1").map(|s| s.name.as_str()),
            Some("Sheet1")
        );
        assert_eq!(
            workbook.sheet("Sheet2").map(|s| s.name.as_str()),
            Some("Sheet2")
        );
        assert!(workbook.sheet("NoSuchSheet").is_none());
    }

    #[test]
    fn sheets_preserve_definition_order() {
        let workbook = Workbook::new(vec![sheet("C"), sheet("A"), sheet("B")]);
        let names: Vec<&str> = workbook.sheets().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["C", "A", "B"]);
    }

    #[test]
    fn empty_workbook() {
        let workbook = Workbook::new(vec![]);
        assert!(workbook.sheets().is_empty());
        assert!(workbook.sheet("anything").is_none());
    }
}
