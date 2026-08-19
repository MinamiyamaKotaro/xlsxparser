// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Pure domain data structures (`Cell`, `Sheet`, `Workbook`, ...). No
//! dependency on XML parsing or resolution logic — see
//! `docs/design/architecture.en.md`.

mod cell;
mod sheet;
mod style;
mod workbook;

pub use cell::{Cell, CellRef, CellValue, DateTimeValue};
pub use sheet::{
    AnchorMarker, ColWidthRange, Image, ImageAnchor, ImageExtent, MergedRegion, Sheet,
    SheetVisibility,
};
pub use style::{Alignment, Font, ResolvedStyle, StyleId, StyleSheet};
pub use workbook::Workbook;
