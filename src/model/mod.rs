//! Pure domain data structures (`Cell`, `Sheet`, `Workbook`, ...). No
//! dependency on XML parsing or resolution logic — see
//! `docs/design/architecture.en.md`.

mod cell;
mod sheet;
mod style;
mod workbook;

pub use cell::{Cell, CellRef, CellValue, DateTimeValue};
pub use sheet::{MergedRegion, Sheet, SheetVisibility};
// `StyleSheet` is unused within the crate until `parse/styles.rs` and
// `resolve/style.rs` (Issue #15) exist to build/consume it.
#[allow(unused_imports)]
pub use style::{ResolvedStyle, StyleId, StyleSheet};
pub use workbook::Workbook;
