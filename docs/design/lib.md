# `lib.rs` 設計書

*[English](lib.en.md)*

`src/lib.rs` に対応する設計書。クレートのルートであり、[architecture.md](architecture.md) が定義する「公開APIのエントリポイント」を担う。`container/` `parse/` `resolve/` `pipeline.rs` `json.rs` を非公開の `mod` として宣言することでクレート内部実装として隠蔽し、`model/` の一部の型と `error::Error` のみを外部へ再エクスポートする。[pipeline.md オープンクエスチョン1・2](pipeline.md) と [json.md オープンクエスチョン6](json.md) が「`lib.rs` の設計時に確定させる」としていた公開API形状をここで確定させる。

## 責務・スコープ

- クレートの公開API関数を定義する: ファイルパスから `.xlsx` をパースする `parse_workbook`、任意の `Read + Seek` から直接パースする `parse_workbook_reader`（いずれも内部で [`pipeline::run`](pipeline.md) を呼び出す薄いラッパー）
- どのサブモジュールをクレート外部へ公開するかを決定する。`container` / `parse` / `resolve` / `pipeline` / `json` は非公開の `mod` として宣言し、クレート内部実装として隠蔽する。個々のファイル内では `pub fn` として定義されている項目（例: [`container::ZipContainer::open`](container/mod.md)）があっても、包含する `mod` 自体が非公開であればRustの可視性規則上クレート外部からは到達不能になる（詳細は依存関係セクション参照）
- `model/` が定義する型のうち、`Workbook` を介して外部から到達しうる型（`Workbook`, `Sheet`, `Cell`, `CellValue`, `CellRef`, `SheetVisibility`, `MergedRegion`, `ResolvedStyle`, `StyleId`, `DateTimeValue`）と `error::{Error, Result}` をクレートルートへ再エクスポートする
- `json::{to_json_writer, to_json_string}` をそのままクレートルートへ再エクスポートし、`Workbook` からJSONへの変換を独立した2段目のステップとして公開する（[pipeline.md オープンクエスチョン1](pipeline.md) の解決を実装する）
- **含まない責務**: パース処理そのもの（`pipeline::run` に委譲）、JSON変換そのもの（`json::to_json_writer`/`to_json_string` に委譲）、公開型の定義そのもの（`model/`）

## 主要な内容（案）

```rust
mod container;
mod error;
mod json;
mod model;
mod parse;
mod pipeline;
mod resolve;

pub use error::{Error, Result};
pub use json::{to_json_string, to_json_writer};
pub use model::{
    Cell, CellRef, CellValue, DateTimeValue, MergedRegion, ResolvedStyle, Sheet,
    SheetVisibility, StyleId, Workbook,
};

use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

/// ファイルパスから `.xlsx` をパースする、最も一般的な公開エントリポイント。
/// 内部で `std::fs::File` を開き [`pipeline::run`](pipeline.md) へ委譲する
/// 薄いラッパー。
pub fn parse_workbook(path: impl AsRef<Path>) -> Result<Workbook> {
    let path = path.as_ref();
    let file = File::open(path).map_err(|source| Error::Io {
        path: Some(path.to_path_buf()),
        source,
    })?;
    pipeline::run(file)
}

/// 任意の `Read + Seek` 入力（インメモリバッファ、HTTPレスポンスボディを
/// 読み切ったバイト列など）から `.xlsx` をパースする。ファイルシステムを
/// 経由しない呼び出し元向けの汎用エントリポイント。ZIP central directory
/// の読み取りにシーク可能性を要求するのは [container/mod.md](container/mod.md)
/// の `ZipContainer::open_reader` の制約をそのまま引き継ぐため（純粋な
/// ストリーミング `Read` のみからは開けない）。
pub fn parse_workbook_reader<R: Read + Seek>(reader: R) -> Result<Workbook> {
    pipeline::run(reader)
}
```

## 依存関係

- 依存先: [`pipeline.rs`](pipeline.md)（`run`）、[`json.rs`](json.md)（`to_json_writer`, `to_json_string`）、[`model/mod.rs`](model/mod.md)（再エクスポートする各型）、[`error.rs`](error.md)（`Error`, `Result`）
- 依存元: なし（クレートの利用者コードのみが本ファイルへ依存する。クレート内の他モジュールは `lib.rs` に依存しない — `pipeline.rs` を含む全モジュールは `crate::model::Workbook` のように `model/` を直接参照し、`crate::Workbook`（`lib.rs` 経由の再エクスポートパス）は経由しない）

`container` / `parse` / `resolve` / `pipeline` / `json` を非公開の `mod` として宣言する設計は、Rustの可視性規則（あるモジュール内の項目の実効的な公開範囲は、その項目自身の可視性と、それを包含する全モジュールの可視性のうち最も狭いもので決まる）を利用している。例えば [`container/mod.md`](container/mod.md) の `ZipContainer::open` / `get_entry` / `entry_names` はいずれも `pub fn` として定義されているが、これは「同じクレート内の `pipeline.rs` から `container::ZipContainer::open(...)` のようにパス修飾して呼べる」ことを保証するためのものであり、`mod container;`（`pub mod` ではない）である限りクレート外部には一切公開されない。[`parse/`](parse/mod.md) 配下の型・関数がほぼ全て `pub(crate)` で宣言されているのも同じ帰結を明示的に書いた設計であり、本ファイルでの非公開 `mod` 宣言と合わせて二重の防御線となる。

[`model/mod.md`](model/mod.md) が `pub use style::{ResolvedStyle, StyleId, StyleSheet};` と `model` モジュールレベルで再エクスポートしている `StyleSheet`（`cellXfs` インデックスから `ResolvedStyle` を引く内部テーブル型）は、本ファイルではさらに外部へ再エクスポートしない。`Cell.style: Option<Arc<ResolvedStyle>>` が公開フィールドとして `ResolvedStyle` 単体を外部から到達可能にする一方、`StyleSheet` 自体（テーブル全体）はどの公開型のフィールドからも参照されないため、クレート内部実装（[`parse/styles.rs`](parse/styles.md) が構築し [`resolve/style.rs`](resolve/style.md) が消費する）にとどめる。

## エラー処理方針

- `parse_workbook` は `std::fs::File::open` の失敗を `Error::Io { path: Some(path), source }` へ変換する。`path` を `Some` にできるのは、ファイルパスという具体的な文脈を本関数自身が持っているためであり、[error.md](error.md) が定義する `Io::path: Option<PathBuf>` の `Some` 側の使用例そのものである
- `parse_workbook_reader` はそれ自身がI/Oエラーを生成する処理を持たない（`reader` は既にメモリ上または呼び出し側が用意した入力であり、本関数はそれを開く処理を行わない）。`pipeline::run` の内部（例えば `container::ZipContainer::open_reader` がZIPとして破損したバイト列を検知した場合）で発生するエラーはそのまま `?` で伝播する。ここで生成されうる `Error::Io` の `path` は `None` となる — [error.md](error.md) が `Io::path: Option<PathBuf>` の設計時に既に想定していた「ファイルパスを経由しない入力、または将来 `lib.rs` が `Read` トレイト入力を受け付ける場合」がまさに本関数に該当する
- 本ファイル自身は新たな `Error` バリアントを生成しない。既存のバリアント（`Io` 以外はすべて `pipeline::run` 以下から伝播する）をそのまま呼び出し元へ返す

## テスト方針

- 正当な `.xlsx` ファイルへのパスを `parse_workbook` に渡した場合に `Ok(Workbook)` が得られることの確認（ファイルシステム経由の統合テスト）
- 存在しないパスを `parse_workbook` に渡した場合に `Error::Io { path: Some(path), .. }` を返すことの確認（`path` が正しく設定されていることを含む）
- 正当な `.xlsx` 相当のバイト列を持つ `std::io::Cursor<Vec<u8>>` を `parse_workbook_reader` に渡した場合に `Ok(Workbook)` が得られることの確認
- 同一の `.xlsx` データに対し `parse_workbook`（ファイル経由）と `parse_workbook_reader`（インメモリ経由）が同じ `Workbook` を返すことの確認（両関数が `pipeline::run` への単純な委譲であることの結線テスト）
- `parse_workbook` の返り値を `to_json_string` にそのまま渡し、有効なJSON文字列が得られることの確認（公開APIの2段構成が実際に連結して動作することを検証するE2Eテスト）
- 破損した `.xlsx`（不正なZIP、必須パーツ欠落など）を渡した場合に、[`pipeline.md`](pipeline.md) が定義する各 `Error` バリアントがそのまま呼び出し元まで伝播することの確認（`lib.rs` 自身が握りつぶしたり別のエラーへ変換したりしないことの確認）
- 公開型（`Workbook`, `Sheet`, `Cell`, `CellValue`, `CellRef`, `SheetVisibility`, `MergedRegion`, `ResolvedStyle`, `StyleId`, `DateTimeValue`, `Error`, `Result`）がクレート外部から `xlsxparser::` 直下の名前として参照可能であることの確認（doctest、または公開API surfaceを固定するテスト。具体的な手法はオープンクエスチョン参照）
- `container` / `parse` / `resolve` / `pipeline` / `json` 配下の型（例: `ZipContainer`, `SharedStringTable`）がクレート外部から到達不可能であること（コンパイルエラーになること）の確認。通常のユニットテストでは「コンパイルできないこと」自体を検証できないため、`trybuild` 等のコンパイル失敗検証クレートの導入を検討する（オープンクエスチョン3参照）

## 未決事項 / オープンクエスチョン

1. **JSON一括変換の利便性関数の要否**: 現状 `parse_workbook` → `to_json_string` という2段呼び出しのみを公開し、両者を1回で行う利便性関数（例: `parse_workbook_json(path) -> Result<String>`）は提供しない設計とした。利用シナリオの多くが「最終的にJSONだけが欲しい」場合であれば、こうした利便性関数を追加する価値があるかは、要求仕様書のフロントエンド利用シナリオの詳細化と合わせて検討の余地がある。
2. **クレート名・パッケージ名**: 本クレートの名前（`Cargo.toml` の `name`、コード例中で `xlsxparser::` として言及している識別子）は要求仕様書にも architecture.md にも明記がなく未確定。`Cargo.toml` 整備時に確定させる。
3. **非公開モジュールの型が外部へ漏れていないことの検証手法**: 依存関係セクションで述べた「非公開 `mod` 宣言による隠蔽」が意図どおり機能しているかを自動テストでどう検証するかは未確定。`trybuild`（コンパイルが失敗することを期待するテスト）の導入、または `cargo public-api` 等の公開APIスナップショットツールによる差分検知が候補だが、いずれも本ライブラリでは前例がなく、採用するかは `Cargo.toml` 整備時に確定させる。
4. **`Sheet` / `Cell` 等のフィールドの公開範囲**: [model/mod.md オープンクエスチョン2](model/mod.md)が「`lib.rs` の公開API設計と合わせて確定させる」としていた論点。本ファイルの設計により「`model/` の主要な型自体を外部へ公開する」という前提は確定したため、残る論点は `MergedRegion`/`CellRef` の `row`/`col` フィールドのように既に `pub` フィールドとして定義済みの粒度が最終的な公開APIとして妥当かの確認のみとなる（現行の型定義をそのまま踏襲する前提でよいと判断する）。
5. **`no_std` 対応の要否**: `parse_workbook`/`parse_workbook_reader` はいずれも `std::fs::File` や `std::io::{Read, Seek}` に依存する。要求仕様書に `no_std` 環境での動作要件はないため現状スコープ外とするが、`container/` `parse/` の設計が `std::io` に強く依存していることも踏まえ、対応する場合は大規模な再設計になる。
