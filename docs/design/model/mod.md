# `model/mod.rs` 設計書

*[English](mod.en.md)*

`src/model/mod.rs` に対応する設計書。`model/` 配下のサブモジュール宣言と、外部（`resolve/` `json.rs` `lib.rs` など）に公開する型の再エクスポートのみを行う集約ファイル。

## 責務・スコープ

- サブモジュールの宣言（`mod cell; mod sheet; mod workbook; mod style;`）
- 公開型の再エクスポート（`pub use cell::{Cell, CellValue, CellRef};` など）
- **含まない責務**: 型定義そのもの（各サブモジュールの責務）、ロジック（`model/` はロジックを持たない純粋データ構造のみという architecture.md の方針上、`mod.rs` にも処理は書かない）

## 主要な内容（案）

```rust
mod cell;
mod sheet;
mod workbook;
mod style;

pub use cell::{Cell, CellRef, CellValue};
pub use sheet::{MergedRegion, Sheet, SheetVisibility};
pub use workbook::Workbook;
pub use style::{ResolvedStyle, StyleId, StyleSheet};
```

`DateTimeValue`（[model/cell.md](cell.md) の未決事項4を参照）は `model/cell.rs` 内で定義済みのため、`cell::{..}` の再エクスポート対象に含まれる想定（具体的な再エクスポート要否は `cell.rs` の実装時に確認）。`ResolvedStyle` / `StyleSheet` / `StyleId` の配置場所は [`model/style.rs`](style.md) 新設により確定した（旧オープンクエスチョン1を解決。PR #8 レビュー指摘を反映）。

## 依存関係

- 依存先: [`model/cell.rs`](cell.md), [`model/sheet.rs`](sheet.md), [`model/workbook.rs`](workbook.md), [`model/style.rs`](style.md)（すべて `mod` 宣言として）
- 依存元: `resolve/`, `parse/`, `json.rs`, `lib.rs` などクレート内の他レイヤー（`crate::model::Workbook` のように本ファイル経由で型を参照する）

## エラー処理方針

対象なし（ロジックを持たないため、エラーを生成・伝播する箇所がない）。

## テスト方針

対象なし。型定義・再エクスポートのみのためユニットテストを持たない。公開APIの構成が意図通りかは `cargo doc` の生成結果、および `lib.rs` からの参照が解決することでビルド時に検証される。

## 未決事項 / オープンクエスチョン

1. ~~`ResolvedStyle` の定義場所~~ → **解決**: [`model/style.rs`](style.md) を新設し定義した（PR #8 レビュー指摘を反映）。`DateTimeValue` は当初から `model/cell.rs` 内定義のプレースホルダーであり、置き場所自体は未決事項ではない（具体的な型は [model/cell.md 未決事項4](cell.md) が別途扱う）。
2. **公開範囲**: `MergedRegion` や `CellRef` のフィールド（`row` / `col`）まで `pub` として外部に公開するか、コンストラクタ経由のみのアクセスに制限するかは、`lib.rs` の公開API設計（別Issue）と合わせて確定させる。
