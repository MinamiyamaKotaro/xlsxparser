# `parse/workbook.rs` 設計書

*[English](workbook.en.md)*

`src/parse/workbook.rs` に対応する設計書。[architecture.md](../architecture.md) が定義する `parse/` の責務のうち「`workbook.xml` のパース」を担う。`xl/workbook.xml` の `<sheets>` 直下に並ぶ `<sheet>` 要素を、ソース定義順を保った一覧へ変換する。[model/workbook.md 未決事項1](../model/workbook.md) が確定させた「`Workbook.sheets` は可視性を問わず全シートを含め、可視性は `SheetVisibility` として保持する」という方針をそのまま実装する。

## 責務・スコープ

- `xl/workbook.xml` をパースし、`<sheets><sheet name="..." sheetId="..." state="..." r:id="..."/></sheets>` の各 `<sheet>` をソース定義順の `Vec<WorkbookSheetEntry>` へ変換する
- `state` 属性（`"visible"` / `"hidden"` / `"veryHidden"`。省略時は可視）を [`model::sheet::SheetVisibility`](../model/sheet.md) へ変換する
- `<workbookPr date1904="1"/>` を `date1904: bool`(Issue #40)としてパースする——[`resolve/style.rs`](../resolve/style.md) が `CellValue::Number` を `CellValue::DateTime` へ変換する際に正しいシリアル値エポックを選ぶために必要とするフラグ。`<workbookPr>` またはその `date1904` 属性が欠落している場合はExcel自身の既定値と同じ `false`(1900日付システム)にフォールバックする。`"1"`/`"true"` のみが真——[`parse/styles.rs`](styles.md) が `<b>`/`<alignment wrapText>` で既に確立した `xsd:boolean` の慣習と同じ
- **含まない責務**: `r:id` を実体ファイルパスへ解決すること（[`parse/relationships.rs`](relationships.md) が構築した `RelationshipMap` と突き合わせるのは `pipeline.rs` の責務）、シート実体（`worksheet.xml`）そのもののパース（[`parse/worksheet.rs`](worksheet.md)）、`model::Sheet` の構築そのもの（`pipeline.rs` が `WorkbookSheetEntry` から `name`/`visibility` を渡して構築する）、`date1904` を消費する実際のシリアル値→暦への変換そのもの([`resolve/style.rs`](../resolve/style.md)——本ファイルはフラグを運ぶのみ)

## 主要な型・関数（案）

```rust
use crate::error::Error;
use crate::model::sheet::SheetVisibility;
use crate::parse::{convert_xml_error, create_secure_reader, required_attr};
use std::io::BufRead;

/// `workbook.xml` の `<sheet>` 1件分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkbookSheetEntry {
    pub name: String,
    /// `<sheets><sheet r:id="rId1" .../></sheets>` の r:id。
    /// `pipeline.rs` が `parse::relationships::RelationshipMap` と突き合わせて
    /// 実体ファイルパスを解決するためのキーとして使う。
    pub r_id: String,
    pub visibility: SheetVisibility,
}

/// `xl/workbook.xml` パース結果: `<sheets>` 直下の `<sheet>` 要素に加え、
/// `<workbookPr>` の `date1904` フラグ(Issue #40)を保持する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedWorkbookXml {
    pub sheets: Vec<WorkbookSheetEntry>,
    pub date1904: bool,
}

/// `xl/workbook.xml` をパースする。`<sheets>` 要素自体が存在しない場合は
/// `Error::MissingRequiredElement` を返す。`<sheets></sheets>` が空の場合は
/// `sheets` が空の `Vec` になる（0シートブックは構造上有効。[model/workbook.md テスト方針](../model/workbook.md) 参照）。
pub(crate) fn parse_workbook_xml(reader: impl BufRead, path: &str) -> Result<ParsedWorkbookXml, Error> {
    let mut xml_reader = create_secure_reader(reader);
    let _ = (&mut xml_reader, path);
    unimplemented!()
}

/// `state` 属性文字列を `SheetVisibility` へ変換する。属性が存在しない場合の
/// 既定値は `Visible`。認識できない値（拡張・破損ファイル由来）は `Visible`
/// へフォールバックする（エラーにしない。オープンクエスチョン3参照）。
fn parse_visibility(state: Option<&str>) -> SheetVisibility {
    match state {
        None | Some("visible") => SheetVisibility::Visible,
        Some("hidden") => SheetVisibility::Hidden,
        Some("veryHidden") => SheetVisibility::VeryHidden,
        Some(_) => SheetVisibility::Visible,
    }
}
```

## 依存関係

- 依存先: [`parse/mod.rs`](mod.md)（`create_secure_reader`, `convert_xml_error`, `required_attr`）、[`model/sheet.rs`](../model/sheet.md)（`SheetVisibility`）、[`error.rs`](../error.md)
- 依存元: `pipeline.rs`（フェーズ1。[`parse/relationships.rs`](relationships.md) が構築した `RelationshipMap` と `r_id` で突き合わせて各シートの実体ファイルパスを決定し、`WorkbookSheetEntry` の `name`/`visibility` から `model::Sheet` を構築したうえで [`parse/worksheet.rs`](worksheet.md) にフェーズ3のストリームパースを委譲する。`ParsedWorkbookXml::date1904` もそのまま運び——`model::Workbook` 自体には保持しない、`StyleSheet` と同じ「フェーズ間の一時値」扱い([architecture.md](../architecture.md)参照)——各シートの `resolve::resolve_sheet` 呼び出しへ渡す）

`date1904` は意図的に公開型 `model::Workbook` のフィールドにはならない([model/workbook.md 未決事項](../model/workbook.md) 参照): フェーズ4で全シートの `resolve_sheet` 呼び出しが消費し終えれば、JSON出力を含む下流の一切が再度必要としないため、`pipeline.rs` のローカル変数のまま保持することで、解決後には使われない値のために `Workbook` の公開APIを肥大化させずに済む。

`parse/workbook.rs` は `model::sheet::SheetVisibility` を直接構築する。architecture.md 設計方針2が禁じるのは「`resolve/` がI/Oやモデル以外に依存すること」であり、`model/` 自体は元々 `parse/` に依存されることを前提とした純粋データ構造の置き場所（[model/sheet.md 依存関係](../model/sheet.md) が `parse/worksheet.rs` を依存元として既に列挙している）であるため、`parse/` から `model/` への依存はこの方針と矛盾しない。

## エラー処理方針

- `<sheets>` 要素自体が存在しない場合、`workbook.xml` として構造的に不正であるため `Error::MissingRequiredElement` を返す
- `<sheet>` の `name` / `r:id` いずれかの属性が欠落している場合は `Error::MissingRequiredElement` を返す
- `date1904` は `<workbookPr>` またはその `date1904` 属性が欠落している場合、あるいは値が `xsd:boolean` の真値表現(`"1"`/`"true"`)以外の場合に `false` へフォールバックする——エラーにはしない。ワークブックの日付システムフラグは、文書の残りの部分を読める状態にするために必須ではないため
- `state` 属性が既知の3値（`visible` / `hidden` / `veryHidden`）以外の場合は `Error::InvalidPackage` 等で拒否せず `Visible` へフォールバックする。可視性は表示上のヒントでありデータの完全性に関わらないため、[resolve/style.md エラー処理方針](../resolve/style.md) が採用する「個々の値解釈の緩やかな失敗はドキュメント全体を失敗させない」という方針と同じ考え方を適用する
- XMLとして構文的に不正な場合は [`convert_xml_error`](mod.md) を通じて `Error::XmlParse` または `Error::ZipBombDetected` に変換する

## テスト方針

- 複数の `<sheet>` を持つ `workbook.xml` から、ソース定義順を保った `Vec<WorkbookSheetEntry>` が得られることの確認
- `state` 属性省略時に `SheetVisibility::Visible` として解釈されることの確認
- `state="hidden"` / `state="veryHidden"` の解釈確認
- `state` 属性が未知の値（例: 将来の仕様拡張や破損ファイル由来の文字列）を持つ場合に、エラーとせず `SheetVisibility::Visible` へフォールバックすることの確認
- `name` 属性・`r:id` 属性のいずれかが欠落した `<sheet>` に対し `Error::MissingRequiredElement` を返すことの確認
- `<sheets>` 要素自体が存在しない `workbook.xml` に対し `Error::MissingRequiredElement` を返すことの確認
- `<sheets></sheets>` が空の場合に空の `Vec` を返すことの確認（0シートブックの結線。[model/workbook.md テスト方針](../model/workbook.md) と対応する）
- **`<workbookPr date1904="1"/>` が `date1904: true` に解決されること、`date1904="0"`・`date1904="true"`・`date1904` 属性の欠落・`<workbookPr>` 要素自体の欠落がそれぞれ期待通りの値に解決されることの確認**(Issue #40)

## 未決事項 / オープンクエスチョン

1. **`sheetId` 属性の扱い**: `<sheet sheetId="1">` はパースするが、[`model::Sheet`](../model/sheet.md) に対応するフィールドがないため現状は破棄する想定。将来的にラウンドトリップ用途等で保持が必要になった場合、`WorkbookSheetEntry` に追加するか別途保持するかは未確定。
2. ~~`r:id` 属性の名前空間解決~~ → **解決**: [parse/mod.md オープンクエスチョン4](mod.md) で確定した「`quick_xml::NsReader` は採用せず文字列前方一致で簡略化する」方針に従い、`"r:id"` という属性名で直接照合する。
3. **`state` 属性の未知の値に対するフォールバック方針の妥当性**: 現状 `Visible` へフォールバックしているが、より安全側（誤って可視化しないよう `Hidden` 扱いにする）にすべきかは、実際の要件・利用ケース次第で未確定。
