# `model/mod.rs` 設計書

*[English](mod.en.md)*

`src/model/mod.rs` に対応する設計書。`model/` 配下のサブモジュール宣言と、外部（`resolve/` `json.rs` `lib.rs` など）に公開する型の再エクスポートのみを行う集約ファイル。

## 責務・スコープ

- サブモジュールの宣言（`mod cell; mod sheet; mod workbook; mod style; mod color;`）
- 公開型の再エクスポート（`pub use cell::{Cell, CellValue, CellRef};` など）
- **含まない責務**: 型定義そのもの（各サブモジュールの責務）、ロジック（`model/` はロジックを持たない純粋データ構造のみという architecture.md の方針上、`mod.rs` にも処理は書かない）

## 主要な内容（案）

```rust
mod cell;
mod sheet;
mod workbook;
mod style;
mod color;

pub use cell::{Cell, CellRef, CellValue};
pub use sheet::{MergedRegion, Sheet, SheetVisibility};
pub use workbook::Workbook;
pub use style::{Alignment, ColorRef, Font, ResolvedStyle, StyleId, StyleSheet};
pub use color::{Rgb, ThemePalette};
```

`Rgb`/`ThemePalette`([`model/color.rs`](color.md)、Issue #76)は、`Workbook::theme()`/`ResolvedStyle.fill_fg_color`等から到達可能な公開APIの一部として再エクスポートする——[`resolve::color::resolve_color`](../resolve/color.md)をクレート外部から直接呼び出す「案A」の呼び出し形（[resolve/color.md](../resolve/color.md)参照）が、これらの型を公開せずには成立しないため。

`DateTimeValue`（[model/cell.md](cell.md) の未決事項4を参照）は `model/cell.rs` 内で定義済みのため、`cell::{..}` の再エクスポート対象に含める。[`lib.md`](../lib.md) の設計により、`CellValue::DateTime` がクレート公開APIの一部である以上 `DateTimeValue` の再エクスポートは必須であることが確定した（具体的な型自体は [model/cell.md 未決事項4](cell.md) が別途扱う）。`ResolvedStyle` / `StyleSheet` / `StyleId` の配置場所は [`model/style.rs`](style.md) 新設により確定した（旧オープンクエスチョン1を解決。PR #8 レビュー指摘を反映）。

## 依存関係

- 依存先: [`model/cell.rs`](cell.md), [`model/sheet.rs`](sheet.md), [`model/workbook.rs`](workbook.md), [`model/style.rs`](style.md), [`model/color.rs`](color.md)（すべて `mod` 宣言として）
- 依存元: `resolve/`, `parse/`, `json.rs`, `lib.rs` などクレート内の他レイヤー（`crate::model::Workbook` のように本ファイル経由で型を参照する）

## エラー処理方針

対象なし（ロジックを持たないため、エラーを生成・伝播する箇所がない）。

## テスト方針

対象なし。型定義・再エクスポートのみのためユニットテストを持たない。公開APIの構成が意図通りかは `cargo doc` の生成結果、および `lib.rs` からの参照が解決することでビルド時に検証される。

## 未決事項 / オープンクエスチョン

1. ~~`ResolvedStyle` の定義場所~~ → **解決**: [`model/style.rs`](style.md) を新設し定義した（PR #8 レビュー指摘を反映）。`DateTimeValue` は当初から `model/cell.rs` 内定義のプレースホルダーであり、置き場所自体は未決事項ではない（具体的な型は [model/cell.md 未決事項4](cell.md) が別途扱う）。
2. ~~公開範囲~~ → **解決**: [`lib.md`](../lib.md) の設計により、`model/` の主要な型自体（`Workbook`/`Sheet`/`Cell`/`CellValue`/`CellRef`/`SheetVisibility`/`MergedRegion`/`ResolvedStyle`/`StyleId`/`DateTimeValue`）を外部へ再エクスポートする方針が確定した。`MergedRegion`/`CellRef` の `row`/`col` フィールドは現行の型定義どおり `pub` のまま踏襲する（[lib.md オープンクエスチョン4](../lib.md)参照）。`StyleSheet` は `Cell` 等の公開フィールドから到達しないため再エクスポートされず、クレート内部実装にとどまる（[lib.md 依存関係](../lib.md)参照）。
