# `lib.rs` 設計書

*[English](lib.en.md)*

`src/lib.rs` に対応する設計書。クレートのルートであり、[architecture.md](architecture.md) が定義する「公開APIのエントリポイント」を担う。`container/` `parse/` `resolve/` `pipeline.rs` `json.rs` を非公開の `mod` として宣言することでクレート内部実装として隠蔽し、`model/` の一部の型と `error::Error` のみを外部へ再エクスポートする。[pipeline.md オープンクエスチョン1・2](pipeline.md) と [json.md オープンクエスチョン6](json.md) が「`lib.rs` の設計時に確定させる」としていた公開API形状をここで確定させる。

## 責務・スコープ

- クレートの公開API関数を定義する: ファイルパスから `.xlsx` をパースする `parse_workbook`、任意の `Read + Seek` から直接パースする `parse_workbook_reader`（いずれも内部で [`pipeline::run`](pipeline.md) を呼び出す薄いラッパー）
- `parse_workbook` は、`pipeline::run` の内部（`File::open` 成功後、ZIP展開やXMLストリーミングの過程）で発生したI/Oエラーの `Error::Io { path: None, .. }` に対し、自身が知っているファイルパスを補完してから呼び出し元へ返す（[PR #11 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/11#pullrequestreview-4949346233)を反映）
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
/// 薄いラッパー。`File::open` 自体の失敗だけでなく、`pipeline::run` の
/// 内部（ZIP展開中やXMLストリーミング中）で発生したI/Oエラーについても、
/// `path` が未設定（`None`）であれば本関数が知っているファイルパスで
/// 補完してから返す（[PR #11 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/11#pullrequestreview-4949346233)
/// を反映）。
pub fn parse_workbook(path: impl AsRef<Path>) -> Result<Workbook> {
    let path = path.as_ref();
    let file = File::open(path).map_err(|source| Error::Io {
        path: Some(path.to_path_buf()),
        source,
    })?;
    pipeline::run(file).map_err(|err| fill_io_path(err, path))
}

/// `pipeline::run` から伝播した `Error::Io { path: None, .. }` に、
/// `parse_workbook` が知っているファイルパスを補完する。それ以外の
/// バリアントはそのまま返す。`Error::XmlParse` / `Error::MissingRequiredElement`
/// も `path` フィールドを持つが、これらはOPCパッケージ内のパーツ名
/// （例: `"xl/worksheets/sheet1.xml"`）を表しファイルシステムパスとは
/// 意味が異なるため補完対象に含めない。
fn fill_io_path(err: Error, path: &Path) -> Error {
    match err {
        Error::Io { path: None, source } => Error::Io {
            path: Some(path.to_path_buf()),
            source,
        },
        other => other,
    }
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
- **`parse_workbook` は `pipeline::run` から返る `Error::Io { path: None, .. }` を `fill_io_path` で補完する**（[PR #11 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/11#pullrequestreview-4949346233)を反映）。`File::open` 自体は成功したがその後のZIP展開・XMLストリーミング中にI/Oエラーが起きた場合（例えば読み取り中にファイルが削除・破損した場合）、`pipeline::run` は `container::ZipContainer` 等がどのファイルパスから読んでいるかを知らないため `path: None` のまま `Error::Io` を返す。`parse_workbook` はこの `None` を自身が保持しているファイルパスで書き換えてから呼び出し元へ返すことで、パース処理のどの段階で発生したI/Oエラーであっても呼び出し元がファイル名を確実に得られるようにする。`Error::XmlParse` / `Error::MissingRequiredElement` の `path` フィールドはOPCパッケージ内のパーツ名（例: `"xl/worksheets/sheet1.xml"`）を表しファイルシステムパスとは意味が異なるため、`fill_io_path` の補完対象には含めない
- `parse_workbook_reader` はそれ自身がI/Oエラーを生成する処理を持たない（`reader` は既にメモリ上または呼び出し側が用意した入力であり、本関数はそれを開く処理を行わない）。`pipeline::run` の内部で発生するエラーはそのまま `?` で伝播する。ここで生成されうる `Error::Io` の `path` は補完されず `None` のままとなる（`parse_workbook` と異なり、本関数はファイルパスという文脈を最初から持たないため補完しようがない） — [error.md](error.md) が `Io::path: Option<PathBuf>` の設計時に既に想定していた「ファイルパスを経由しない入力」がまさに本関数に該当する
- 本ファイル自身は新たな `Error` バリアントを生成しない。既存のバリアント（`Io` 以外はすべて `pipeline::run` 以下から伝播する）をそのまま呼び出し元へ返す。`fill_io_path` は既存の `Error::Io` インスタンスの `path` フィールドを書き換えるのみで、新しいバリアントを生成しない

## テスト方針

- 正当な `.xlsx` ファイルへのパスを `parse_workbook` に渡した場合に `Ok(Workbook)` が得られることの確認（ファイルシステム経由の統合テスト）
- 存在しないパスを `parse_workbook` に渡した場合に `Error::Io { path: Some(path), .. }` を返すことの確認（`path` が正しく設定されていることを含む）
- **`fill_io_path` 単体に対し、`Error::Io { path: None, .. }` を渡した場合に `path` が `Some` へ書き換わること、`Error::Io { path: Some(..), .. }` や他バリアント（`Error::XmlParse` 等）を渡した場合は変更されずそのまま返ることの確認**（PR #11 レビューで追加した補完仕様の単体テスト。`pipeline::run` の内部で実際にファイル読み取り中のI/Oエラーを再現させる統合テストはファイルシステム操作のタイミングに依存し不安定になりやすいため、`fill_io_path` 単体のテストで代替する）
- 正当な `.xlsx` 相当のバイト列を持つ `std::io::Cursor<Vec<u8>>` を `parse_workbook_reader` に渡した場合に `Ok(Workbook)` が得られることの確認
- 同一の `.xlsx` データに対し `parse_workbook`（ファイル経由）と `parse_workbook_reader`（インメモリ経由）が同じ `Workbook` を返すことの確認（両関数が `pipeline::run` への単純な委譲であることの結線テスト）
- `parse_workbook` の返り値を `to_json_string` にそのまま渡し、有効なJSON文字列が得られることの確認（公開APIの2段構成が実際に連結して動作することを検証するE2Eテスト）
- 破損した `.xlsx`（不正なZIP、必須パーツ欠落など）を渡した場合に、[`pipeline.md`](pipeline.md) が定義する各 `Error` バリアントがそのまま呼び出し元まで伝播することの確認（`lib.rs` 自身が握りつぶしたり別のエラーへ変換したりしないことの確認）
- 公開型（`Workbook`, `Sheet`, `Cell`, `CellValue`, `CellRef`, `SheetVisibility`, `MergedRegion`, `ResolvedStyle`, `StyleId`, `DateTimeValue`, `Error`, `Result`）がクレート外部から `xlsxparser::` 直下の名前として参照可能であることの確認（doctest、または公開API surfaceを固定するテスト。具体的な手法はオープンクエスチョン参照）
- `container` / `parse` / `resolve` / `pipeline` / `json` 配下の型（例: `ZipContainer`, `SharedStringTable`）がクレート外部から到達不可能であること（コンパイルエラーになること）の確認。通常のユニットテストでは「コンパイルできないこと」自体を検証できないため、`trybuild` 等のコンパイル失敗検証クレートの導入を検討する（オープンクエスチョン3参照）

## 未決事項 / オープンクエスチョン

1. **JSON一括変換の利便性関数の要否**: 現状 `parse_workbook` → `to_json_string` という2段呼び出しのみを公開し、両者を1回で行う利便性関数（例: `parse_workbook_json(path) -> Result<String>`）は提供しない設計とした。利用シナリオの多くが「最終的にJSONだけが欲しい」場合であれば、こうした利便性関数を追加する価値があるかは、要求仕様書のフロントエンド利用シナリオの詳細化と合わせて検討の余地がある。
2. ~~クレート名・パッケージ名~~ → **解決**: `xlsxparser`（本ファイルの実装に先立ち、CI整備のためのクレート雛形を作った際に `Cargo.toml` で確定済み。Issue #16）。
3. **非公開モジュールの型が外部へ漏れていないことの検証手法**: 未確定のまま、本ファイルの初回実装では対応していない。`crate::container::ZipContainer` 等が実際にクレート外から到達不能なのは、再エクスポートしうる各モジュールが `pub(crate)`／非公開 `mod` の可視性を守っているという設計によるものであり、現時点では目視でのレビュー（依存関係セクション）のみで確認しており、自動的なコンパイル失敗テストや公開APIの差分検知は行っていない。将来的に自動検証が必要になった場合、`trybuild` / `cargo public-api` が引き続き候補となる。
4. ~~`Sheet` / `Cell` 等のフィールドの公開範囲~~ → **解決**: 本ファイルの設計が想定していた通り、既存の `pub` フィールドの粒度（`MergedRegion`/`CellRef` の `row`/`col`、`Cell` の `value`/`style` 等）をそのまま実装へ踏襲した。
5. **`no_std` 対応の要否**: `parse_workbook`/`parse_workbook_reader` はいずれも `std::fs::File` や `std::io::{Read, Seek}` に依存する。要求仕様書に `no_std` 環境での動作要件はないため現状スコープ外とするが、`container/` `parse/` の設計が `std::io` に強く依存していることも踏まえ、対応する場合は大規模な再設計になる。
