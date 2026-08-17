//! `xlsxparser` — a lightweight, high-performance `.xlsx` (OOXML) parser library.
//!
//! Implementation in progress per Issue #15; modules are added one at a time
//! following `docs/design/architecture.en.md`. The public entry points
//! described in `docs/design/lib.en.md` (`parse_workbook`, ...) land once
//! every module they depend on exists.

mod container;
mod error;
mod model;
mod parse;
mod resolve;

pub use error::{Error, Result};
pub use model::{
    Cell, CellRef, CellValue, DateTimeValue, MergedRegion, ResolvedStyle, Sheet, SheetVisibility,
    StyleId, Workbook,
};
