# `model/mod.rs` 設計書

*[English](mod.en.md)*

`src/model/mod.rs` に対応する設計書。`model/` 配下のサブモジュール宣言と、外部（`resolve/` `json.rs` `lib.rs` など）に公開する型の再エクスポートのみを行う集約ファイル。

## 責務・スコープ

- サブモジュールの宣言（`mod cell; mod sheet; mod workbook;`）
- 公開型の再エクスポート（`pub use cell::{Cell, CellValue, CellRef};` など）
- **含まない責務**: 型定義そのもの（各サブモジュールの責務）、ロジック（`model/` はロジックを持たない純粋データ構造のみという architecture.md の方針上、`mod.rs` にも処理は書かない）

## 主要な内容（案）

```rust
mod cell;
mod sheet;
mod workbook;

pub use cell::{Cell, CellRef, CellValue};
pub use sheet::{MergedRegion, Sheet};
pub use workbook::Workbook;
```

`ResolvedStyle`（[model/cell.md](cell.md) の未決事項1を参照）をどのファイルに定義するかは未確定だが、`model/` 配下に置く場合は本ファイルでの再エクスポート対象に加わる。

## 依存関係

- 依存先: [`model/cell.rs`](cell.md), [`model/sheet.rs`](sheet.md), [`model/workbook.rs`](workbook.md)（すべて `mod` 宣言として）
- 依存元: `resolve/`, `json.rs`, `lib.rs` などクレート内の他レイヤー（`crate::model::Workbook` のように本ファイル経由で型を参照する）

## エラー処理方針

対象なし（ロジックを持たないため、エラーを生成・伝播する箇所がない）。

## テスト方針

対象なし。型定義・再エクスポートのみのためユニットテストを持たない。公開APIの構成が意図通りかは `cargo doc` の生成結果、および `lib.rs` からの参照が解決することでビルド時に検証される。

## 未決事項 / オープンクエスチョン

1. **`ResolvedStyle` の定義場所**: [model/cell.md](cell.md) の未決事項1で触れた通り、`Cell.style` の型を `model/` 側に置くか `resolve/style.rs` 側に置くかが未決定であり、それに伴い本ファイルでの再エクスポート対象も変わる。
2. **公開範囲**: `MergedRegion` や `CellRef` のフィールド（`row` / `col`）まで `pub` として外部に公開するか、コンストラクタ経由のみのアクセスに制限するかは、`lib.rs` の公開API設計（別Issue）と合わせて確定させる。
