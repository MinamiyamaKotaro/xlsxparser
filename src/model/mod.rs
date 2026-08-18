//! Pure domain data structures (`Cell`, `Sheet`, `Workbook`, ...). No
//! dependency on XML parsing or resolution logic — see
//! `docs/design/architecture.en.md`.

mod cell;
mod sheet;
mod style;
mod workbook;

pub use cell::{Cell, CellRef, CellValue, DateTimeValue};
pub use sheet::{ColWidthRange, MergedRegion, Sheet, SheetVisibility};
pub use style::{ResolvedStyle, StyleId, StyleSheet};
pub use workbook::Workbook;
