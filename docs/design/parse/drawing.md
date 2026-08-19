# `parse/drawing.rs` 設計書

*[English](drawing.en.md)*

`src/parse/drawing.rs` に対応する設計書。Issue [#65](https://github.com/MinamiyamaKotaro/xlsxparser/issues/65)(「画像のアンカー位置・リンク先を取得できない」)のうち、純粋なXMLパース部分を担う: `xl/drawings/drawingN.xml` の `xdr:twoCellAnchor`/`xdr:oneCellAnchor` 要素を `PendingImage` にパースする。各要素が持つ `r:embed`/ハイパーリンクの `r:id` の解決や、`drawingN.xml` 自体をワークシート自身の `_rels` 経由で特定する処理は `pipeline.rs` の責務([pipeline.md](../pipeline.md) のPhase 3.5参照)であり、[relationships.md](relationships.md) が既に確立していた「ルーティング用データのパース」と「その解釈・解決」の分業に倣う。

## 責務・スコープ

- `xl/drawings/drawingN.xml` の `xdr:twoCellAnchor`/`xdr:oneCellAnchor` 要素をパースする。各要素は `<xdr:pic>`(埋め込み画像)をセル位置に紐付ける
- 各アンカーについて以下を抽出する:
  - `xdr:from`/`xdr:to` マーカー(`TwoCell`)または `xdr:from`/`xdr:ext`(`OneCell`) — セル座標とEMU単位のオフセット。DrawingMLの0始まりの `xdr:col`/`xdr:row` から本クレートの1始まりの `CellRef` へ変換する
  - `<xdr:pic>` の `r:embed`(埋め込みメディアのrelationship ID)、および存在すれば `a:hlinkClick` の `r:id`(画像自体のハイパーリンクのrelationship ID) — まだターゲットパスに解決されていない生の文字列として取得する
- `<xdr:pic>` を持たないアンカー(単純な図形・グラフのアンカー)は無視する(何も返さない)。本Issueのスコープ外
- **含まない責務**: `embed_r_id`/`hyperlink_r_id` を `drawingN.xml.rels` に対して解決すること(`pipeline.rs` の責務。本モジュールは渡された単一のreader以外に2つ目のZIPエントリを開いたり、いかなるI/Oも行わない)、どの `drawingN.xml` がどのワークシートに属するかの特定(`pipeline.rs` が、ワークシート自身の `_rels` と、`parse/worksheet.rs` が新たに収集するようになった `<drawing r:id="...">` 要素経由で行う)、埋め込み画像自体のバイト列を読むこと(Issue全体としてスコープ外 — Issue本文の理由: 差分検出用途のツールにピクセルデータは不要であり、読み込むとメモリ使用量がセル数ではなく画像数に比例してしまう)

## 主要な型・関数

```rust
use crate::error::Error;
use crate::model::{AnchorMarker, CellRef, ImageAnchor, ImageExtent};
use crate::parse::{create_secure_reader, optional_attr, read_event, read_leaf_text, required_attr};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::BufRead;

/// `twoCellAnchor`/`oneCellAnchor` 内の `<xdr:pic>` 1個分。relationship ID
/// はまだ実際のターゲットパスに解決されていない。`pipeline.rs` が
/// `embed_r_id`/`hyperlink_r_id` を `drawingN.xml.rels` に対して解決し、
/// `model::Image` に変換する。
pub(crate) struct PendingImage {
    pub anchor: ImageAnchor,
    pub embed_r_id: String,
    pub hyperlink_r_id: Option<String>,
}

/// drawingN.xml 1個分を、それが持つ全ての `<xdr:pic>` にパースする。
pub(crate) fn parse_drawing(reader: impl BufRead, path: &str) -> Result<Vec<PendingImage>, Error> {
    // 各 <xdr:twoCellAnchor>/<xdr:oneCellAnchor> について、その
    // <xdr:from>/<xdr:to>/<xdr:ext> マーカーをパースし、<xdr:pic> が
    // 存在すれば <a:blip r:embed>/<a:hlinkClick r:id> も取得する。
    // <xdr:pic> を持たないアンカーはスキップする(内部的にOk(None)として
    // 扱われ、結果からは単に除外される)。
    ..
}
```

`AnchorMarker`/`ImageExtent`/`ImageAnchor`/`Image` 自体は [`model/sheet.rs`](../model/sheet.md) に `MergedRegion`/`ColWidthRange` と並んで定義されている — 本モジュールは `model::Image` にまだ無い部分(未解決の生のrelationship ID)のみを生成する。これは `parse/worksheet.rs` が解決済みの `Cell` を直接生成するのではなく `PendingSharedString`/`PendingStyle` を生成するのと同じパターンである。

### 0始まりから1始まりへの変換

DrawingMLの `xdr:col`/`xdr:row` は0始まり(ECMA-376 Part 1の `ST_ColumnRow` — `CT_Marker` の祖先)だが、本クレートの `CellRef` はA1形式に合わせて1始まりである([`model/cell.md`](../model/cell.md) 参照)。`zero_based_to_cell_ref` は `CellRef` を構築する前に各値へ1を加算し、変換後に `u32` をオーバーフローする、または `CellRef::MAX_ROW`/`MAX_COL` を超える値は `Error::InvalidCellRef` として拒否する。これは `CellRef::from_a1` 自身の範囲チェックと同じ理由(セキュリティレビュー `docs/security/code-review.md` Finding 2)によるもので、どの経路から来た座標であってもXML由来の攻撃者制御可能な値が未検証のままモデルに到達してはならない、という原則に従う。

## 依存関係

- 依存先: [`parse/mod.rs`](mod.md)(`create_secure_reader`, `read_event`, `read_leaf_text`, `required_attr`, `optional_attr`)、[`model/sheet.rs`](../model/sheet.md)(`AnchorMarker`, `ImageAnchor`, `ImageExtent`)、[`model/cell.rs`](../model/cell.md)(`CellRef`)、[`error.rs`](../error.md)
- `read_leaf_text` は元々 `parse/worksheet.rs` 内のprivateヘルパーだったが、本モジュールからも使うために `parse/mod.rs` の共有関数へ昇格させた — 両モジュールとも「ネストした要素を想定しない」単純な数値・テキストのleaf要素(`<v>`, `<xdr:col>` 等)を読む点で共通しており、`concat_rich_text` が扱うより複雑な `<r><t>` ラン構造とは性質が異なる
- 依存元: `pipeline.rs` のPhase 3.5([pipeline.md](../pipeline.md) 参照)。`PendingImage` のrelationship IDを `drawingN.xml.rels` に対して解決し、最終的な `Vec<model::Image>` を構築する

## エラー処理方針

- `<xdr:pic>` を持つアンカーにおいて必須要素(`TwoCell` アンカーの `xdr:from`/`xdr:to`、`OneCell` アンカーの `xdr:from`/`xdr:ext`、`<xdr:pic>` の `<a:blip>` が持つべき `r:embed`)が欠落している場合は `Error::MissingRequiredElement` — `parse/worksheet.rs` が `<c>` の `r` 属性欠落に適用するのと同じfail-fast方針
- `<xdr:pic>` を全く持たないアンカーは上記チェックが走る前に早期リターンし、結果から単に除外される — 単純な図形・グラフのアンカーがこれらを持たないのは正当なため、エラーとしない
- leaf要素の数値内容が不正な場合(`xdr:col`/`xdr:colOff`/`xdr:row`/`xdr:rowOff`、または `xdr:ext` の `cx`/`cy` 属性)は `Error::InvalidPackage` — `parse/worksheet.rs::parse_u32_attr`/`parse_f64_attr` の規約(整形式の要素だが期待する型としてパースできない内容を持つ場合)に倣う
- 0始まりから1始まりへの変換でオーバーフローする、または `CellRef::MAX_ROW`/`MAX_COL` を超える座標は `Error::InvalidCellRef`(上記「主要な型・関数」参照)
- 構文的に不正なXMLは、他の `parse/` モジュールと同じ `create_secure_reader`/`read_event` のゲートウェイを経由して `Error::XmlParse`/`Error::ZipBombDetected`/`Error::DoctypeRejected` に変換される

## テスト方針

- `<a:blip r:embed>` と `<a:hlinkClick r:id>` の両方を持つ `twoCellAnchor` が、両方のIDを捕捉し、`from`/`to` マーカーが正しく1始まりの `CellRef` とEMUオフセットに変換された `PendingImage` にパースされることの確認
- `<xdr:ext>` を持ちハイパーリンクを持たない `oneCellAnchor` が、`hyperlink_r_id: None` かつ `ext` の `cx`/`cy` が保持された `PendingImage` にパースされることの確認
- `<xdr:pic>` を持たないアンカー(例: 単なる `<xdr:sp>`)がスキップされることの確認 — `from`/`to` 自体は整形式であっても `PendingImage` を生成せず、エラーにもならない
- 複数のアンカーを持つ `drawingN.xml` から、画像アンカーの数だけ文書順に `PendingImage` が生成されることの確認
- `<a:blip r:embed>` を欠く `<xdr:pic>` が `Error::MissingRequiredElement { name: "r:embed", .. }` になることの確認
- 1始まり変換後に `u32` をオーバーフローする、または `CellRef::MAX_ROW`/`MAX_COL` を超える `xdr:row`/`xdr:col` の値が `Error::InvalidCellRef` になることの確認
- `xdr:ext` の属性が不正な場合(`cx`/`cy` が数値でない)に `Error::InvalidPackage` になることの確認
- アンカーを一切持たない空の `<xdr:wsDr>` が空の `Vec` を返すことの確認

## 未決事項 / オープンクエスチョン

1. **図形・グラフ等、画像以外の描画オブジェクト**: 現状は静かにスキップされる(`PendingImage` を生成しない)。将来これらも出力モデルに反映する必要が生じた場合(例: `Image` とは別の汎用的な「図形」アンカーとして)、本モジュールのアンカーごとのループにもう一つの返却経路が必要になる — Issue #65の明示的なスコープが画像のみであるため、本設計では扱わない。
2. **`editAs` 等のアンカー挙動属性**: `xdr:twoCellAnchor` の `editAs` 属性(`twoCell`/`oneCell`/`absolute` — 元となるセルがリサイズされた際の図形の挙動)は取得していない。これはExcelの*動的な*リサイズ挙動に影響するものであり、本ライブラリの出力(差分検出用途)が関心を持つアンカーの*現在の*位置・サイズとは別の関心事である — ただし将来「画像そのものが移動した」のか「周囲のセルがリサイズされて画像が追従した」のかを区別する必要が生じた場合は再検討の余地がある。
3. ~~`parse/relationships.rs` がメディア埋め込み用relsに対応する必要があるか~~ → **解決**: [relationships.md オープンクエスチョン1](relationships.md) で未確定だった論点に、Issue #65が回答を与えた — `parse/relationships.rs` の既存の汎用 `_rels` パーサー(`../media/image1.png` のような相対パスに対して既にテスト済み)を、`xl/worksheets/_rels/sheetN.xml.rels`(`drawingN.xml` の特定)と `xl/drawings/_rels/drawingN.xml.rels`(埋め込みメディア・ハイパーリンクのターゲット特定)の両方にそのまま再利用し、当該モジュールへの変更は不要だった。
