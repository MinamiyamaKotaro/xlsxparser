# `parse/relationships.rs` 設計書

*[English](relationships.en.md)*

`src/parse/relationships.rs` に対応する設計書。[architecture.md](../architecture.md) が定義するフェーズ1「`_rels` 解析（ルーティングマップ構築用データのパース）」を担う。`xl/_rels/workbook.xml.rels` 等、OPC (Open Packaging Conventions) の `_rels/*.rels` パーツ共通のXML構造（`<Relationships><Relationship .../></Relationships>`）を解析する。[container/mod.md 依存関係](../container/mod.md) が「`parse/relationships.rs`（フェーズ1）が `.rels` 内の相対パス表記とエントリ名を組み合わせて動的に計算する」と既に前提としていたターゲットパスの解決も本ファイルの責務とする。

## 責務・スコープ

- `_rels/*.rels` パーツのXMLをパースし、`r:id`（Relationship ID）をキーとした `RelationshipMap` を構築する
- 各 `<Relationship>` の `Target` 属性（relsパーツ自身からの相対パス表記、例: `worksheets/sheet1.xml`, `../media/image1.png`）を、そのrelsパーツが属するディレクトリを起点にZIPエントリ名相当の絶対パス（例: `xl/worksheets/sheet1.xml`）へ解決する
- `TargetMode="External"`（外部URIを指す関係）を内部パーツと区別する。本ライブラリはアーカイブ外リソースをフェッチしないため、Externalな関係のターゲット解決（パス正規化）は行わずURI文字列をそのまま保持する
- **含まない責務**: どの `r:id` がどのOOXMLパーツ種別（worksheet/sharedStrings/styles等）に対応するかの意味づけ・フィルタリング（呼び出し元の `pipeline.rs`。本ファイルはあらゆる `_rels` パーツを汎用的に解析するのみで、`Relationship.rel_type` の文字列を解釈しない）、解決したターゲットパスが実際にZIPアーカイブ内に存在するかの検証（[`container::ZipContainer::get_entry`](../container/mod.md) が `Ok(None)` として表現する）

## 主要な型・関数（案）

```rust
use crate::error::Error;
use crate::parse::{convert_xml_error, create_secure_reader, required_attr};
use std::collections::HashMap;
use std::io::BufRead;

/// 個々の `<Relationship>` 要素。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Relationship {
    /// r:id（例: "rId1"）。
    pub id: String,
    /// Type属性のフルURI（例: ".../relationships/worksheet"）。文字列のまま
    /// 保持し、意味づけは呼び出し元（pipeline.rs）に委ねる（オープンクエスチョン3参照）。
    pub rel_type: String,
    /// Internalの場合は `resolve_target_path` で解決済みのZIPエントリ名相当の
    /// 絶対パス。Externalの場合はTarget属性のURI文字列をそのまま保持する。
    pub target: String,
    pub target_mode: TargetMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetMode {
    /// 既定値（TargetMode属性省略時）。ZIPアーカイブ内部のパーツを指す。
    Internal,
    /// 外部URI（http(s) 等）を指す。本ライブラリはフェッチしない。
    External,
}

/// r:id から `Relationship` を引くルーティングマップ。
pub(crate) type RelationshipMap = HashMap<String, Relationship>;

/// `_rels` パーツのXML（例: `xl/_rels/workbook.xml.rels` の中身）をパースし、
/// `RelationshipMap` を構築する。
///
/// `part_dir` は、このrelsパーツが結びつく対象パーツのディレクトリ
/// （例: `xl/workbook.xml` に対応するrelsなら `"xl"`）。`Target` の相対パス
/// 解決の起点として使う。`path` はエラーメッセージ用の識別子
/// （rels自身のZIPエントリ名。例: `"xl/_rels/workbook.xml.rels"`）。
pub(crate) fn parse_relationships(
    reader: impl BufRead,
    part_dir: &str,
    path: &str,
) -> Result<RelationshipMap, Error> {
    let mut xml_reader = create_secure_reader(reader);
    let _ = (&mut xml_reader, part_dir, path);
    // 実装方針: <Relationship>要素ごとにId/Type/Target/TargetMode属性を
    // required_attr等で取得し、TargetMode=Internal（既定）の場合のみ
    // resolve_target_pathでtargetを解決する。
    unimplemented!()
}

/// `base_dir`（relsパーツが結びつく対象パーツのディレクトリ）を起点に、
/// rels内の相対パス表記 `target` を解決し、ZIPエントリ名としての絶対パス
/// を返す。OPCのパート名は常に `/` 区切りであるため `std::path::Path` は
/// 使わず、文字列をセグメント単位で手動処理することでOS依存のパス解釈
/// （Windowsの `\` 区切りなど）を避ける。
///
/// `..` セグメントは直前のセグメントを取り除く（親ディレクトリ参照）ことで
/// 素朴に処理するが、`base_dir` の深さを超える `..`（例: `base_dir` が
/// `"xl"` で `target` が `"../../evil"`）に対する挙動はこの関数単体では
/// 保証しない（依存関係セクション参照）。
fn resolve_target_path(base_dir: &str, target: &str) -> String {
    let mut segments: Vec<&str> = base_dir.split('/').filter(|s| !s.is_empty()).collect();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            seg => segments.push(seg),
        }
    }
    segments.join("/")
}
```

## 依存関係

- 依存先: [`parse/mod.rs`](mod.md)（`create_secure_reader`, `convert_xml_error`, `required_attr`）、[`error.rs`](../error.md)
- 依存元: `pipeline.rs`（フェーズ1で `xl/_rels/workbook.xml.rels` を解析し、シートID・共有文字列・スタイルの各実体パーツへのルーティングマップを構築する。architecture.md 「フェーズ1完了時にルーティングマップ構築後、`_rels` の一時バッファを破棄する」に従い、`RelationshipMap` 自体はフェーズ1完了後に破棄される想定）

`resolve_target_path` が `base_dir` の深さを超える `..` を受け取った場合、`Vec::pop()` は空のベクタに対して単に何もしない（`None` を返すだけ）ため、意図しない浅いパス（最悪の場合は空文字列）を生成しうる。これは [container/mod.md 依存関係](../container/mod.md) が既に述べていた「`get_entry` に渡される `name` は `parse/relationships.rs` が動的に計算した値でありうり、この計算過程に正規化漏れがあった場合に備えて `get_entry` が独立して再検証する」という多層防御の前提そのものである。したがって本関数はこの種の異常な入力を積極的に拒否せず、最終的な安全性は [`container::ZipContainer::get_entry`](../container/mod.md) が呼び出しのたびに行う `validate_entry_path` の再検証に委ねる（オープンクエスチョン2参照）。

## エラー処理方針

- `<Relationship>` の `Id` / `Type` / `Target` いずれかの属性が欠落している場合は `Error::MissingRequiredElement` を返す（`TargetMode` は省略可・既定値 `Internal`）
- XMLとして構文的に不正な場合は [`convert_xml_error`](mod.md) を通じて `Error::XmlParse` または `Error::ZipBombDetected` に変換する
- `resolve_target_path` 自体はエラーを返さない（`panic` せず、常に何らかの文字列を返す）。異常なパスの最終防御は呼び出し側（`container::get_entry`）に委ねる設計（依存関係セクション参照）

## テスト方針

- 複数の `<Relationship>` を持つ正当な `_rels` XMLから期待どおりの `RelationshipMap`（`id` をキーとした内容）が得られることの確認
- `Id` / `Type` / `Target` のいずれかを欠く `<Relationship>` に対し `Error::MissingRequiredElement` を返すことの確認
- `resolve_target_path`: 単純な相対パス（`"worksheets/sheet1.xml"`）が `base_dir` と結合され正しく絶対パス化されることの確認
- `resolve_target_path`: 親ディレクトリ参照を含む相対パス（`base_dir = "xl/worksheets"`, `target = "../media/image1.png"` → `"xl/media/image1.png"`）が正しく解決されることの確認
- `resolve_target_path`: `base_dir` の深さを超える `..`（例: `base_dir = "xl"`, `target = "../../evil"`）を渡しても本関数自体は `panic` せず何らかの文字列を返すこと、および実際にそのようなパスを渡した経路が最終的に `container::get_entry` の再検証で `Error::ZipSlipDetected` として拒否されることの確認（[container/mod.md](../container/mod.md) との結線を確認する回帰テスト）
- `TargetMode="External"` を持つ `<Relationship>` の `target` が `resolve_target_path` を経由せず、`Target` 属性の文字列がそのまま保持されることの確認
- 子要素を持たない空の `<Relationships>` に対し空の `RelationshipMap` を返すことの確認

## 未決事項 / オープンクエスチョン

1. ~~本ファイルが解析対象とする `_rels` パーツの範囲~~ → **解決**: 元々は特定パーツに限定しない汎用的な `_rels` パーサーとして設計しており、メディア埋め込み対応の要否は未確定だった。Issue [#65](https://github.com/MinamiyamaKotaro/xlsxparser/issues/65) がこれに回答した: `xl/worksheets/_rels/sheetN.xml.rels` と `xl/drawings/_rels/drawingN.xml.rels` にも既存の汎用パーサーをそのまま再利用し、本ファイルへの変更は不要だった([drawing.md](drawing.md) 参照)。`[Content_Types].xml` のrels対応は依然として未確定。
2. **`resolve_target_path` の過剰な `..` に対する扱い**: 現状は本関数自身ではエラーにせず、最終防御を `container::get_entry` の再検証に委ねる設計（多層防御）とした。`segments` が空になった、または `base_dir` の外側へ抜けたことが本関数の時点で明確に判定できるケースについて、ここで早期に `Error::ZipSlipDetected` 相当として拒否すべきかは、多層防御の各層の責務分担として要検討。
3. **`Relationship.rel_type` の型**: 現状フルURI文字列（`String`）のまま保持しているが、呼び出し元（`pipeline.rs`）が既知の関係タイプ（worksheet/sharedStrings/styles等）を判定する際に文字列比較を都度行うことになる。既知タイプを表す `enum` を事前定義し変換するかは、`pipeline.rs` の設計時にあわせて確定させる。
4. ~~名前空間の扱い~~ → **解決**: [parse/mod.md オープンクエスチョン4](mod.md) で確定した「`quick_xml::NsReader` は採用せず文字列前方一致で簡略化する」方針に従う。`_rels` XML自体は固定の名前空間（`http://schemas.openxmlformats.org/package/2006/relationships`）を持つが、要素名・属性名（`Relationship`, `Id`, `Type`, `Target`, `TargetMode`）に接頭辞は付かないため、本ファイルへの影響は他モジュールに比べ限定的である。
