// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Single-cell data: `CellRef` (A1 coordinate), `CellValue`, `Cell`.

use crate::error::Error;
use crate::model::style::ResolvedStyle;
use std::sync::Arc;

/// A resolved date/time value, decomposed into its calendar fields by
/// `resolve/style.rs::serial_to_date_time` from an Excel date serial number
/// (Issue #40). A plain `Copy` value type — no sub-second precision, since
/// Excel serial values (`f64`) can't reliably represent it beyond float
/// rounding noise, and no downstream use case has asked for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DateTimeValue {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Cell coordinates. 1-based, matching Excel (A1 = row:1, col:1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CellRef {
    pub row: u32,
    pub col: u32,
}

impl CellRef {
    /// Excel's real maximum row (1,048,576) and column (16,384, "XFD") —
    /// ECMA-376 Part 1 §18.3.1.35 / ISO 29500. A coordinate beyond this can
    /// never come from genuine Excel output; `from_a1` rejects it rather
    /// than accepting it as "valid", since letting it through would
    /// propagate an attacker-controlled bound into `Sheet::max_row`/
    /// `max_col` and, from there, into `json.rs`'s `maxRow`/`maxCol`
    /// output — which a downstream consumer might trust to size a dense
    /// grid (security review `docs/security/code-review.md` Finding 2).
    pub const MAX_ROW: u32 = 1_048_576;
    pub const MAX_COL: u32 = 16_384;

    /// Builds from an "A1"-style string (uppercase column letters followed by
    /// a decimal row number, e.g. `"B12"`). Returns `Err` for anything else:
    /// lowercase letters, mixed/extra symbols, a missing letter or digit
    /// part, a row/column number that overflows `u32`, is `0`, or exceeds
    /// Excel's real maximum ([`Self::MAX_ROW`]/[`Self::MAX_COL`]).
    pub fn from_a1(s: &str) -> Result<Self, Error> {
        let invalid = || Error::InvalidCellRef(s.to_string());

        let split_at = s.find(|c: char| c.is_ascii_digit()).ok_or_else(invalid)?;
        let (letters, digits) = s.split_at(split_at);

        if letters.is_empty() || !letters.chars().all(|c| c.is_ascii_uppercase()) {
            return Err(invalid());
        }
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            return Err(invalid());
        }

        let col = column_letters_to_number(letters).ok_or_else(invalid)?;
        let row: u32 = digits.parse().map_err(|_| invalid())?;
        if row == 0 || row > Self::MAX_ROW || col > Self::MAX_COL {
            return Err(invalid());
        }

        Ok(CellRef { row, col })
    }

    /// Converts to an "A1"-style string.
    pub fn to_a1(&self) -> String {
        format!("{}{}", column_number_to_letters(self.col), self.row)
    }
}

/// Converts a 1-based column number to its bijective base-26 letter form
/// (1 -> "A", 26 -> "Z", 27 -> "AA", ...).
fn column_number_to_letters(mut n: u32) -> String {
    let mut buf = Vec::new();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        buf.push(b'A' + rem);
        n = (n - 1) / 26;
    }
    buf.reverse();
    String::from_utf8(buf).expect("column letters are always ASCII")
}

/// Converts uppercase column letters to a 1-based column number, returning
/// `None` on overflow. Assumes `letters` is non-empty and all ASCII
/// uppercase (callers validate this beforehand).
fn column_letters_to_number(letters: &str) -> Option<u32> {
    let mut n: u32 = 0;
    for c in letters.chars() {
        let digit = (c as u32) - ('A' as u32) + 1;
        n = n.checked_mul(26)?.checked_add(digit)?;
    }
    Some(n)
}

/// A cell's value. Has a variant corresponding to each OOXML `t` attribute
/// (cell type).
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    /// The default when the `t` attribute is omitted. Non-date serial values
    /// live here.
    Number(f64),
    /// Converted from `Number` when `resolve/style.rs` determines, from the
    /// numFmt, that the value is a date/time.
    DateTime(DateTimeValue),
    /// A resolved string (shared string `t="s"` / inline str / str are all
    /// unified into this form once resolved). Uses `Arc<str>` to avoid
    /// duplicate allocations across cells that share the same string.
    Text(Arc<str>),
    Boolean(bool),
    /// `t="e"`. Holds the error code string (e.g. `"#DIV/0!"`) as-is.
    Error(String),
}

/// A single entry in the sparse matrix. Only cells that hold data or
/// formatting exist in `Sheet` (blank cells are not instantiated).
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// `Option` so that a cell with formatting only (no value) can be
    /// represented.
    pub value: Option<CellValue>,
    /// `None` represents the default (unset) style. `Arc` avoids duplicating
    /// identical styles across cells and decouples cell lifetime from the
    /// `StyleSheet` container's lifetime.
    pub style: Option<Arc<ResolvedStyle>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_a1_to_a1_round_trip() {
        let cases = [
            ("A1", CellRef { row: 1, col: 1 }),
            ("Z1", CellRef { row: 1, col: 26 }),
            ("AA1", CellRef { row: 1, col: 27 }),
            (
                "XFD1048576",
                CellRef {
                    row: 1_048_576,
                    col: 16_384,
                },
            ),
        ];

        for (a1, expected) in cases {
            let parsed = CellRef::from_a1(a1).unwrap();
            assert_eq!(parsed, expected, "parsing {a1}");
            assert_eq!(parsed.to_a1(), a1, "round-trip of {a1}");
        }
    }

    #[test]
    fn from_a1_rejects_invalid_strings() {
        let invalid = [
            "a1",              // lowercase
            "A#1",             // mixed symbols
            "A1B2",            // symbols/order mixed in the row part
            "A",               // column-only
            "123",             // row-only
            "",                // empty
            "A0",              // row 0 is out of range
            "A10000000000000", // row overflows u32
            "A1048577",        // row exceeds Excel's real maximum (1,048,576) by 1
            "XFE1",            // column exceeds Excel's real maximum (16,384 = XFD) by 1
        ];

        for s in invalid {
            assert!(CellRef::from_a1(s).is_err(), "expected {s:?} to be invalid");
        }
    }

    #[test]
    fn from_a1_rejects_row_or_col_far_beyond_excels_real_maximum() {
        // Security review docs/security/code-review.md Finding 2: a
        // coordinate like this parsed successfully before this bound was
        // added, and its row/col flowed unclamped into Sheet::max_row/
        // max_col and from there into json.rs's maxRow/maxCol output — a
        // downstream consumer trusting those to size a dense grid could be
        // driven into the same kind of allocation failure this project's
        // own benchmarking observed calamine hit (README.md "Benchmarks").
        let err = CellRef::from_a1("ZZZZZZ4294967294").unwrap_err();
        assert!(matches!(err, Error::InvalidCellRef(_)));
    }

    #[test]
    fn cell_value_equality() {
        assert_eq!(CellValue::Number(1.5), CellValue::Number(1.5));
        assert_ne!(CellValue::Number(1.5), CellValue::Number(2.5));
        let dt = DateTimeValue {
            year: 2024,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        };
        assert_eq!(CellValue::DateTime(dt), CellValue::DateTime(dt));
        assert_ne!(
            CellValue::DateTime(dt),
            CellValue::DateTime(DateTimeValue { day: 2, ..dt })
        );
        assert_eq!(
            CellValue::Text(Arc::from("hello")),
            CellValue::Text(Arc::from("hello"))
        );
        assert_ne!(
            CellValue::Text(Arc::from("hello")),
            CellValue::Text(Arc::from("world"))
        );
        assert_eq!(CellValue::Boolean(true), CellValue::Boolean(true));
        assert_ne!(CellValue::Boolean(true), CellValue::Boolean(false));
        assert_eq!(
            CellValue::Error("#DIV/0!".into()),
            CellValue::Error("#DIV/0!".into())
        );
        assert_ne!(CellValue::Number(1.0), CellValue::Boolean(true));
    }

    #[test]
    fn cell_with_formatting_only_has_no_value() {
        let style = Arc::new(ResolvedStyle {
            is_date_time: false,
            ..Default::default()
        });
        let cell = Cell {
            value: None,
            style: Some(style.clone()),
        };
        assert_eq!(cell.value, None);
        assert_eq!(cell.style, Some(style));
    }

    #[test]
    fn arc_sharing_avoids_duplication() {
        let style = Arc::new(ResolvedStyle {
            is_date_time: true,
            ..Default::default()
        });
        let a = Cell {
            value: None,
            style: Some(style.clone()),
        };
        let b = Cell {
            value: None,
            style: Some(style.clone()),
        };
        assert!(Arc::ptr_eq(
            a.style.as_ref().unwrap(),
            b.style.as_ref().unwrap()
        ));

        let text: Arc<str> = Arc::from("shared");
        let a = Cell {
            value: Some(CellValue::Text(text.clone())),
            style: None,
        };
        let b = Cell {
            value: Some(CellValue::Text(text.clone())),
            style: None,
        };
        let (Some(CellValue::Text(a_text)), Some(CellValue::Text(b_text))) = (&a.value, &b.value)
        else {
            unreachable!()
        };
        assert!(Arc::ptr_eq(a_text, b_text));
    }
}
