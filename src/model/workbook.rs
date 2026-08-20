// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! `Workbook`: the final resolved data model, returned by `parse_workbook`.

use crate::model::color::ThemePalette;
use crate::model::sheet::Sheet;

/// The final resolved output model. The return value of
/// `lib.rs::parse_workbook`.
#[derive(Debug, Clone)]
pub struct Workbook {
    sheets: Vec<Sheet>,
    /// The 12-color theme palette resolved from the `xl/theme/theme{N}.xml`
    /// part. `None` for a workbook without the part at all, or one whose
    /// `StyleSheet` never references a `ColorRef::Theme` (Issue #76's
    /// pay-for-what-you-use optimization — `pipeline::run` never even reads
    /// the part in that case). Pass this to
    /// `resolve::color::resolve_color` to resolve a `ColorRef::Theme` on
    /// `ResolvedStyle.fill_fg_color`/`fill_bg_color` to a real `Rgb`.
    theme: Option<ThemePalette>,
}

impl Workbook {
    /// Builds a `Workbook` from a list of already-resolved sheets and a
    /// theme palette. `pipeline.rs` calls this exactly once, after Phases 3
    /// and 4 have completed for every sheet.
    pub(crate) fn new(sheets: Vec<Sheet>, theme: Option<ThemePalette>) -> Self {
        Self { sheets, theme }
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

    /// The theme palette, if present. Pass straight through as the `theme`
    /// argument to `resolve::color::resolve_color` when resolving a
    /// `ColorRef::Theme` to a real RGB value.
    pub fn theme(&self) -> Option<&ThemePalette> {
        self.theme.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::color::Rgb;
    use crate::model::sheet::SheetVisibility;

    fn sheet(name: &str) -> Sheet {
        Sheet::new(name.to_string(), SheetVisibility::Visible)
    }

    #[test]
    fn sheet_lookup_by_name() {
        let workbook = Workbook::new(vec![sheet("Sheet1"), sheet("Sheet2")], None);
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
        let workbook = Workbook::new(vec![sheet("C"), sheet("A"), sheet("B")], None);
        let names: Vec<&str> = workbook.sheets().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["C", "A", "B"]);
    }

    #[test]
    fn empty_workbook() {
        let workbook = Workbook::new(vec![], None);
        assert!(workbook.sheets().is_empty());
        assert!(workbook.sheet("anything").is_none());
    }

    #[test]
    fn theme_is_none_when_workbook_has_no_theme_part() {
        let workbook = Workbook::new(vec![sheet("Sheet1")], None);
        assert!(workbook.theme().is_none());
    }

    #[test]
    fn theme_returns_the_palette_when_present() {
        let palette = ThemePalette([Rgb::default(); 12]);
        let workbook = Workbook::new(vec![sheet("Sheet1")], Some(palette));
        assert_eq!(workbook.theme(), Some(&palette));
    }
}
