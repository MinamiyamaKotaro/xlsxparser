# `json.rs` 設計書

*[English](json.en.md)*

`src/json.rs` に対応する設計書。[architecture.md](architecture.md) が定義するフェーズ5「JSON生成（返却）」を担う。分析・解決が完了した [`model::Workbook`](model/workbook.md) を、フロントエンド描画に必要な `row_span` / `col_span` などの属性を含むJSONへシリアライズする（要求仕様書5章）。

## 責務・スコープ

- [`model::Workbook`](model/workbook.md) を、`row_span`/`col_span` や値の種別タグを含むJSONへシリアライズする
- [`Sheet::iter_cells`](model/sheet.md)（起点セルのみを走査）が返すイテレータから1セルずつ直接シリアライザへ書き出し、シート全体分の中間 `Vec` をヒープ上に構築しない（[PR #10 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/10#pullrequestreview-4949223332)を反映してオープンクエスチョン5を解決。要求仕様書が主眼とする「方眼紙Excel」規模のシートに対するピークメモリ抑制）
- [`Sheet::merged_region_at`](model/sheet.md) を用いて `row_span`/`col_span` を算出し、結合セルの仮想セル座標をJSON出力へ含めない（要求仕様書3.2、5章の実装）
- [`CellValue`](model/cell.md) の各バリアントに応じて、JSON上の値と種別タグ（`type: "number" | "text" | "boolean" | "error" | "dateTime"`。値を持たない、または表現不能な値は `"empty"`）を出力する
- **含まない責務**: モデルデータの解決・検証そのもの（`resolve/`。本ファイルに到達する時点で `Workbook` は全フェーズの検証を通過済みの正常データのみを保持する）、`to_json_writer` に渡す `Write` 実装そのものの用意（ファイルオープン・HTTPレスポンスの確保等は呼び出し側の責務）

## 主要な型・関数（案）

```rust
use crate::error::Error;
use crate::model::cell::{Cell, CellRef, CellValue};
use crate::model::sheet::{Sheet, SheetVisibility};
use crate::model::style::Alignment;
use crate::model::workbook::Workbook;
use serde::ser::{SerializeSeq, SerializeStruct};
use serde::{Serialize, Serializer};
use std::io::Write;

/// `workbook` をJSONとして `writer` へストリーミング出力する。`cells` 配列の
/// 各要素は [`Sheet::iter_cells`](model/sheet.md) から都度1件ずつ変換・
/// 書き出され、シート全体分の `Vec<JsonCell>` を中間バッファとして保持しない。
/// `writer` が例えば `BufWriter<File>` であれば、追加メモリ使用量は
/// 処理中のセル1件分（O(1)）に抑えられる。
pub fn to_json_writer<W: Write>(workbook: &Workbook, writer: W) -> Result<(), Error> {
    let json_workbook = JsonWorkbook { workbook };
    serde_json::to_writer(writer, &json_workbook)
        .map_err(|source| Error::JsonSerialize { source: Box::new(source) })
}

/// `to_json_writer` をインメモリの `Vec<u8>` に対して呼び出す簡易版。
/// 出力全体を1つの `String` として保持する必要があるため、追加メモリ使用量は
/// 出力サイズに比例したO(n)となる（`to_json_writer` のO(1)とは異なる点に
/// 注意。ファイル・HTTPレスポンス等へ直接書き出せる場合は `to_json_writer`
/// を使うことを推奨する）。
pub fn to_json_string(workbook: &Workbook) -> Result<String, Error> {
    let mut buf = Vec::new();
    to_json_writer(workbook, &mut buf)?;
    // serde_jsonは常に妥当なUTF-8を出力することが保証されているため、
    // ここでの変換は理論上失敗しないが、ライブラリ内部で`unwrap`/`expect`を
    // 使わない方針（error.mdエラー処理方針）に従い`Result`のまま扱う。
    String::from_utf8(buf).map_err(|source| Error::JsonSerialize { source: Box::new(source) })
}

/// `model::Workbook` への借用ラッパー。値を所有せず、`Serialize` 実装内で
/// 都度モデルを走査することでストリーミングを実現する（このファイルの
/// 外へは公開しない）。
struct JsonWorkbook<'a> {
    workbook: &'a Workbook,
}

impl<'a> Serialize for JsonWorkbook<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Workbook", 1)?;
        state.serialize_field("sheets", &SheetSeq { workbook: self.workbook })?;
        state.end()
    }
}

struct SheetSeq<'a> {
    workbook: &'a Workbook,
}

impl<'a> Serialize for SheetSeq<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let sheets = self.workbook.sheets();
        let mut seq = serializer.serialize_seq(Some(sheets.len()))?;
        for sheet in sheets {
            seq.serialize_element(&JsonSheet { sheet })?;
        }
        seq.end()
    }
}

struct JsonSheet<'a> {
    sheet: &'a Sheet,
}

impl<'a> Serialize for JsonSheet<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Sheet", 7)?;
        state.serialize_field("name", &self.sheet.name)?;
        state.serialize_field("visibility", visibility_tag(self.sheet.visibility))?;
        state.serialize_field("maxRow", &self.sheet.max_row)?;
        state.serialize_field("maxCol", &self.sheet.max_col)?;
        // `defaultColumnWidth`/`columns`（Issue #39）: 列単位の値を
        // その列の全セルへ複製するのではなく、シート単位の配列として
        // 出力する——理由は model/sheet.md「機能: 列幅」の注記参照
        // （Issue #36のレビュー議論で提起）。
        state.serialize_field("defaultColumnWidth", &self.sheet.default_col_width())?;
        state.serialize_field("columns", &ColumnSeq { sheet: self.sheet })?;
        state.serialize_field("cells", &CellSeq { sheet: self.sheet })?;
        state.end()
    }
}

/// [`Sheet::iter_cells`](model/sheet.md) から1セルずつ `JsonCell` へ変換し
/// つつ直接シリアライザへ書き出す。中間の `Vec<JsonCell>` を構築しない
/// （オープンクエスチョン5を解決した設計の中核）。
struct CellSeq<'a> {
    sheet: &'a Sheet,
}

impl<'a> Serialize for CellSeq<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Sheet::iter_cellsは`impl Iterator`としてのみ公開されておりサイズ
        // ヒントを保証しないため`None`とする（`ExactSizeIterator`をpublic
        // APIとして約束するかはmodel/sheet.md側の設計判断に委ねる。
        // オープンクエスチョン5参照）。
        let mut seq = serializer.serialize_seq(None)?;
        for (cell_ref, cell) in self.sheet.iter_cells() {
            seq.serialize_element(&cell_to_json(self.sheet, cell_ref, cell))?;
        }
        seq.end()
    }
}

/// 1セル分の変換結果。`CellSeq::serialize` がストリームの1要素として
/// 都度生成する、短命な値（呼び出し元へは公開しない）。
#[derive(Debug, Serialize)]
struct JsonCell {
    row: u32,
    col: u32,
    value: JsonCellValue,
    /// 1（結合されていない）の場合はフィールド自体を省略する。
    #[serde(rename = "rowSpan", skip_serializing_if = "is_one")]
    row_span: u32,
    #[serde(rename = "colSpan", skip_serializing_if = "is_one")]
    col_span: u32,
    /// セルがスタイルを一切持たない(`Cell.style: None`)場合はフィールド
    /// 自体を省略する。`columns`(シート単位の配列。model/sheet.md
    /// 「機能: 列幅」の注記参照)とは異なり、フォントは同じ列内でも
    /// セルごとに本当に変わりうるため、セル単位に埋め込んでも疎な出力
    /// という原則を破らない(Issue #38)。
    #[serde(skip_serializing_if = "Option::is_none")]
    style: Option<JsonStyle>,
}

fn is_one(n: &u32) -> bool {
    *n == 1
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonStyle {
    font: JsonFont,
    wrap_text: bool,
    /// `font`/`wrap_text` と同様、常に出力される(`Option` にしない)——
    /// `numberFormat` とは異なり「General」はExcelの配置モードとして意味の
    /// ある実在の値であり、「報告すべき情報なし」ではない(Issue #42)。
    alignment: &'static str,
    /// `None`(「General」——特別な書式なし。`model/style.rs` の
    /// `ResolvedStyle::number_format` ドキュメントコメント参照)の場合は
    /// フィールド自体を省略する——`style` オブジェクトが存在する限り常に
    /// 意味のある値を持つ `font`/`wrap_text`/`alignment` とは異なる
    /// (Issue #41)。
    #[serde(skip_serializing_if = "Option::is_none")]
    number_format: Option<String>,
    /// `<fill>`に`<fgColor>`/`<bgColor>`が一切無い場合は省略する
    /// (Issue #75)——`number_format`と同じ「報告すべき情報なし」の扱い。
    #[serde(skip_serializing_if = "Option::is_none")]
    fill_fg_color: Option<JsonColorRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fill_bg_color: Option<JsonColorRef>,
    /// どの辺にも罫線が無い場合(`Borders::any()`が`false`——ほとんどの
    /// セル)は省略する。`{"top":false,"right":false,"bottom":false,
    /// "left":false}`としては出力しない(Issue #97)——`fillFgColor`と
    /// 同じ「報告すべき情報なし」の扱い。
    #[serde(skip_serializing_if = "Option::is_none")]
    borders: Option<JsonBorders>,
}

/// `Borders`のJSON形式(Issue #97)——`JsonColorRef`と異なり区別すべき
/// バリアントを持たない、単純な非タグ付きオブジェクト。オブジェクト
/// 自体が存在する場合、4つのフィールドは常に一緒に出力される
/// (`rowSpan`/`colSpan`の「単一値の全部か無しか」の省略と同型であり、
/// `fillFgColor`/`fillBgColor`のフィールド単位の省略とは異なる——辺ごとの
/// `false`はここでは「報告すべき情報なし」ではなく意味のある情報のため)。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonBorders {
    top: bool,
    right: bool,
    bottom: bool,
    left: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonFont {
    size_pt: f64,
    bold: bool,
}

/// `ColorRef`のJSON形式(Issue #75)。`JsonCellValue`と同じタグ付け方式
/// ——例: `{"type":"theme","value":{"index":4,"tint":-0.25}}`。
/// `model::ColorRef`自身と同様、生の指定のまま保持する。
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
enum JsonColorRef {
    Rgb(String),
    Theme { index: u32, tint: Option<f64> },
    Indexed(u32),
}

/// 種別タグ付きの値表現。`#[serde(tag = "type", content = "value")]` により
/// `{"type": "number", "value": 42.0}` の形でシリアライズされる。
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
enum JsonCellValue {
    Number(f64),
    /// ISO 8601、ミリ秒なし固定(`DateTimeValue` はミリ秒以下の精度を
    /// 持たない)、例: `"2024-01-01T13:45:30"`(Issue #40。
    /// オープンクエスチョン3を解決)。日付のみのセルも時刻00:00:00として
    /// 出力する——Excel自体が日付/時刻の精度を型として区別しないため。
    DateTime(String),
    Text(std::sync::Arc<str>),
    Boolean(bool),
    Error(String),
    /// 値を持たない（書式のみの）セル、または非有限浮動小数点数
    /// （後述）などJSONで表現不能な値のフォールバック先。
    Empty,
}

fn cell_to_json(sheet: &Sheet, cell_ref: CellRef, cell: &Cell) -> JsonCell {
    let (row_span, col_span) = sheet
        .merged_region_at(cell_ref)
        .map(|r| (r.row_span(), r.col_span()))
        .unwrap_or((1, 1));
    JsonCell {
        row: cell_ref.row,
        col: cell_ref.col,
        value: cell_value_to_json(cell.value.as_ref()),
        row_span,
        col_span,
        style: cell.style.as_ref().map(|s| JsonStyle {
            font: JsonFont { size_pt: s.font.size_pt, bold: s.font.bold },
            wrap_text: s.wrap_text,
            alignment: alignment_tag(s.horizontal_alignment),
            number_format: s.number_format.as_deref().map(str::to_string),
            fill_fg_color: s.fill_fg_color.as_ref().map(color_ref_to_json),
            fill_bg_color: s.fill_bg_color.as_ref().map(color_ref_to_json),
            borders: borders_to_json(&s.borders),
        }),
    }
}

fn borders_to_json(b: &Borders) -> Option<JsonBorders> {
    b.any().then_some(JsonBorders {
        top: b.top,
        right: b.right,
        bottom: b.bottom,
        left: b.left,
    })
}

fn cell_value_to_json(value: Option<&CellValue>) -> JsonCellValue {
    match value {
        None => JsonCellValue::Empty,
        Some(CellValue::Number(n)) if n.is_finite() => JsonCellValue::Number(*n),
        // NaN/Infinityを0.0へ静かに置き換えると、下流の集計処理が
        // 「正常に0と評価された」値と区別できず誤った集計結果を招きうる
        // （会計・業務システム向けという要求仕様書1章の利用文脈を踏まえ、
        // PR #10レビュー指摘を反映してオープンクエスチョン2を解決）。
        // 値なし（Empty/JSON上のnull相当）へフォールバックし、フロント
        // エンドが「値が存在しない」ものとして安全に扱えるようにする。
        Some(CellValue::Number(_)) => JsonCellValue::Empty,
        Some(CellValue::DateTime(dt)) => JsonCellValue::DateTime(format_date_time(dt)),
        Some(CellValue::Text(s)) => JsonCellValue::Text(s.clone()),
        Some(CellValue::Boolean(b)) => JsonCellValue::Boolean(*b),
        Some(CellValue::Error(e)) => JsonCellValue::Error(e.clone()),
    }
}

/// `DateTimeValue` をタイムゾーン指定子・ミリ秒なしのISO 8601形式へ変換する。
/// 例: `"2024-01-01T13:45:30"`。
fn format_date_time(dt: &crate::model::cell::DateTimeValue) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
    )
}

/// `model::style::Alignment` には直接 `Serialize` を導出しない(`serde` を
/// `model/` の依存へ持ち込まない方針を維持する——下記の依存関係参照)ため、
/// `visibility_tag` と同じパターンで変換する(Issue #42)。
fn alignment_tag(a: Alignment) -> &'static str {
    match a {
        Alignment::General => "general",
        Alignment::Left => "left",
        Alignment::Center => "center",
        Alignment::Right => "right",
        Alignment::Fill => "fill",
        Alignment::Justify => "justify",
        Alignment::CenterContinuous => "centerContinuous",
        Alignment::Distributed => "distributed",
    }
}

fn visibility_tag(v: SheetVisibility) -> &'static str {
    match v {
        SheetVisibility::Visible => "visible",
        SheetVisibility::Hidden => "hidden",
        SheetVisibility::VeryHidden => "veryHidden",
    }
}
```

## 依存関係

- 依存先: [`model/workbook.rs`](model/workbook.md)（`Workbook`）、[`model/sheet.rs`](model/sheet.md)（`Sheet::iter_cells`, `Sheet::merged_region_at`, `Sheet::images`, `SheetVisibility`, `Image`, `ImageAnchor`, `AnchorMarker`——Issue #65）、[`model/cell.rs`](model/cell.md)（`Cell`, `CellRef`, `CellValue`, `DateTimeValue`）、[`model/style.rs`](model/style.md)（`Alignment`——`cell_to_json` 内で `s.horizontal_alignment` として読み取り、直接 `Serialize` を導出せず `alignment_tag` を介して変換する。下記の「`model/` に `serde` を持ち込まない」方針と同じ。`ColorRef`——`s.fill_fg_color`/`fill_bg_color` として読み取り、同様に `color_ref_to_json` を介して変換する。`Borders`——`s.borders` として読み取り、4つの`bool`フィールドを`JsonBorders`へそのままコピーする。`bool`は`ColorRef`/`Alignment`のような`model`→JSON変換関数を必要としない）、[`error.rs`](error.md)（`Error::JsonSerialize`。ストリーミング書き込み時のI/O・シリアライズ失敗を表現するため新設。[PR #10 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/10#pullrequestreview-4949223332)を踏まえた設計変更に伴い追加）、外部クレート `serde`（`Serialize` の手動・導出実装）・`serde_json`（`to_writer` によるストリーミングシリアライズ）。`serde` は `rc` フィーチャの有効化が必要（実装時に判明: `CellValue::Text` の `Arc<str>` フィールドは、このフィーチャを有効にしないと `Serialize` を実装しない。serde は `Rc`/`Arc` のシリアライズを既定では無効にしており、これは共有データが独立したシリアライズ呼び出しごとに黙って複製されてしまう落とし穴を避けるための設計）。
- 依存元: `lib.rs`（`Workbook` から明示的に呼び出す。[pipeline.md オープンクエスチョン1](pipeline.md) 参照。`pipeline.rs` の `run` 自体からは呼ばれない）

`JsonWorkbook` / `SheetSeq` / `JsonSheet` / `CellSeq` はいずれもモデルへの借用（`&'a Workbook` / `&'a Sheet`）のみを保持し、値を所有しない。`Serialize` 実装は呼び出された時点で初めてモデルを走査するため、`serde_json::to_writer` が内部で行う逐次的なシリアライズ呼び出しと自然に噛み合い、シート全体・ブック全体を表す中間データ構造をヒープ上に一切構築しない。

`model::Workbook` / `Sheet` / `Cell` に直接 `#[derive(Serialize)]` を付与せず、本ファイル専用の借用ラッパー型へ変換してからシリアライズする設計とした。理由は [error.md](error.md) が `Error::XmlParse::source` を型消去して `quick-xml` をパブリック依存にしないようにした設計判断と同じ構造で、`model/` に `serde` への依存を持ち込むと `serde` の破壊的変更が `model/` 側の型定義に波及しうる。ラッパー層を挟むことで、`model/` は architecture.md の方針どおり「XMLパースや解決ロジックに依存しない純粋なデータ構造」のまま保たれ、JSON出力の具体的なフィールド名・形状を `model/` の型定義から独立して変更できる。

## エラー処理方針

- `to_json_writer` / `to_json_string` は `Result<_, Error>` を返す。`serde_json::to_writer` は非有限浮動小数点数（`NaN`/`Infinity`）を検知すると `Err` を返す仕様だが、本ファイルは `cell_value_to_json` の時点で非有限な `f64` を必ず `JsonCellValue::Empty` へ変換してから `serde_json` へ渡すため、この経路でのエラーは実質的に発生しない。それでも `Result` を返す設計としているのは、(1) `writer` が `File` やネットワークソケット等I/Oを伴う実装の場合、書き込み自体が失敗しうるため、(2) ライブラリ内部で `unwrap`/`expect` を使わない（[error.md エラー処理方針](error.md)）という既存方針を守るためである
- `serde_json::Error` は具体的な型を `Error` のフィールドへ直接置かず `Box<dyn std::error::Error + Send + Sync + 'static>` として型消去した新設バリアント `Error::JsonSerialize` へ包む。理由は [error.md](error.md) の `XmlParse::source` と同一で、`serde_json` をパブリック依存にしないため
- 唯一JSONが表現できない値である `f64` の `NaN`/`Infinity`（`CellValue::Number` が理論上保持しうる）は、`Err` を返して呼び出し元にドキュメント全体のシリアライズを諦めさせるのではなく、当該セルのみ `JsonCellValue::Empty`（JSON上の `null` 相当）へフォールバックし、処理を継続する。`0.0` のような有効な数値へ静かに置き換えないのは、会計・業務システム向けという要求仕様書1章の利用文脈において「計算失敗・未定義値」と「正常に評価されたゼロ」が下流の集計処理で区別できなくなることを避けるため（[PR #10 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/10#pullrequestreview-4949223332)を反映）

## テスト方針

- 単一シート・単一セル（数値）を持つ `Workbook` が `to_json_string` で期待するJSON文字列へ変換されることの確認
- 結合セルを持つシートで、起点セルの `rowSpan`/`colSpan` が正しく算出され、仮想セル座標が `cells` 配列に含まれないことの確認（`Sheet::iter_cells` が起点セルのみを返す設計との結線）
- 結合されていない通常セルで `rowSpan`/`colSpan` フィールドがシリアライズ結果から省略される（`skip_serializing_if`）ことの確認
- `CellValue` の各バリアント（`Number`/`Text`/`Boolean`/`Error`）が対応する `type` タグと `value` で正しくシリアライズされることの確認
- `value: None`（書式のみのセル）が `type: "empty"` としてシリアライズされることの確認
- **`CellValue::Number(f64::NAN)` / `CellValue::Number(f64::INFINITY)` を持つセルが `Err` を返さず `type: "empty"` として出力されることの確認**（PR #10 レビューで追加したフォールバック仕様の回帰テスト観点。従来の `0.0` へのフォールバックとの違いを明示するテスト）
- 可視性が `Hidden`/`VeryHidden` のシートを含む `Workbook` で、全シートが `visibility` フィールド付きで出力に含まれることの確認（[model/workbook.md](model/workbook.md) の「非表示シートも全て含める」方針との結線）
- 0シートの `Workbook` が `{"sheets": []}` として正しくシリアライズされることの確認
- **多数のセルを持つシートに対し `to_json_writer` を呼び出した際、`Sheet` 自体のメモリ使用量に対して追加のヒープ確保が有意に増加しない（`JsonCell` のVec化が行われていない）ことを検証する回帰テスト**（PR #10 レビューで指摘されたピークメモリ抑制の設計意図を裏付けるテスト。具体的な検証手法はメモリプロファイリングツールの選定と合わせて実装時に確定させる）
- `to_json_writer` に書き込み途中で失敗する `Write` 実装（テスト用のモック）を渡した場合に `Error::JsonSerialize` が伝播することの確認
- **`Sheet::col_width_ranges`/`default_col_width` がシート単位の `columns` 配列/`defaultColumnWidth` フィールドとしてシリアライズされ、個々のセルオブジェクトに複製されないことの確認**（Issue #39。「セルごとではなくシート単位の配列」という設計判断そのものを検証する。model/sheet.md参照）
- **`Sheet::images` がシート単位の `images` 配列としてシリアライズされること、`TwoCell` アンカーが `{"type":"twoCell","from":...,"to":...}`、`OneCell` アンカーが `{"type":"oneCell","from":...,"ext":...}` を出力すること、画像がハイパーリンクを持たない場合は `hyperlink` が(`null` ではなく)省略されることの確認**（Issue #65）
- **スタイルを持つセルの `font`(`size_pt`/`bold`)がセル単位の `style` オブジェクトの下にネストしてシリアライズされ、スタイルを持たないセル(`Cell.style: None`)では `style` フィールド自体が省略されることの確認**（Issue #38。フォントは同じ列内でもセルごとに本当に変わりうるため、`columns` とは逆の疎性判断になる）
- **`style.wrapText` が `style.font` と同じセル単位の `style` オブジェクトの下に、`true`/`false` いずれの場合もシリアライズされることの確認**（Issue #37。`JsonStyle` が常に両フィールドを一緒に持つようになったため、`font` で確立済みの「スタイルあり/なし」の疎性配線をそのまま再利用する）
- **`CellValue::DateTime` を持つセルが `{"type": "dateTime", "value": "..."}` としてISO 8601文字列でシリアライズされ、一桁の暦フィールドがゼロ埋めされることの確認**(例: `2024-01-05T03:05:09` であって `2024-1-5T3:5:9` ではない——Issue #40)
- **解決済みの `number_format` を持つスタイル付きセルが `style.numberFormat` をその文字列としてシリアライズすること、および `number_format: None`(「General」)のスタイル付きセルは `style` 自体は存在するのに `numberFormat` フィールドだけが省略されることの確認**(Issue #41。同じ既存の `style` オブジェクト内で `font`/`wrap_text` とは逆の疎性判断になる——「General」は下流にとって不要な情報のため)
- **`style.alignment` が常に出力され(省略されない)、各 `Alignment` バリアントが対応するcamelCase文字列でシリアライズされること(既定値の `"general"` を含む)の確認**(Issue #42。`numberFormat` ではなく `font`/`wrap_text` と同じ「常に出力」の疎性判断)
- **`ColorRef::Rgb`/`Theme`/`Indexed` を持つスタイル付きセルが、`JsonCellValue`と同じタグ付け方式で `style.fillFgColor`/`fillBgColor` をシリアライズすること(例: `{"type":"rgb","value":"FFFF0000"}`)、`tint`の無い`Theme`は`tint`をJSON `null` としてシリアライズすること(省略されるのは外側の`fillFgColor`/`fillBgColor`キー自体のみ)、塗りつぶし色を持たないセルは両フィールドとも完全に省略されることの確認**(Issue #75。`numberFormat` と同じ「`font`/`wrap_text` とは逆の疎性判断」)
- **`Borders { top: true, ... }`(いずれかの辺が`true`)を持つセルが`style.borders`を`{"top":true,"right":false,"bottom":false,"left":false}`としてシリアライズすること(4つのキー全てが一緒に出力され、`false`の辺だけが個別に省略されることは無い)、`Borders::default()`(どの辺も無し)のセルは`style`から`borders`キー自体が完全に省略されることの確認**(Issue #97)

## 未決事項 / オープンクエスチョン

1. ~~JSON構造における値の種別タグ付けの是非~~ → **解決**: `{"type": "number", "value": 42}` のようなタグ付き表現を維持する（[PR #10 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/10#pullrequestreview-4949223332)を反映）。タグを外してネイティブなJSON型のみで出力すると、`dateTime` と単なる文字列（`Text`）をフロントエンド側が区別できず日付ピッカー等の適用に文字列解析が必要になること、`error`（数式エラー値）を通常の文字列と区別してグリッド上で警告表示するといった制御ができなくなること、TypeScript側でタグ付きユニオン型（Discriminated Union）による型安全なクライアント実装ができなくなることが理由。
2. ~~非有限浮動小数点数（`NaN`/`Infinity`）のフォールバック値~~ → **解決**: `0.0` ではなく `JsonCellValue::Empty`（`null` 相当）へフォールバックする（[PR #10 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/10#pullrequestreview-4949223332)を反映）。詳細はエラー処理方針参照。
3. ~~`DateTime` の文字列表現形式~~ → **解決**(Issue #40): タイムゾーン指定子・ミリ秒なしのISO 8601形式、例: `"2024-01-01T13:45:30"`。日付のみのセルも時刻00:00:00(`T00:00:00`)としてシリアライズする——省略するのではなく。Excel自体が日付のみ/日付+時刻を型として区別しないため元々それ以上の情報が存在せず、セルごとに形式が変わるフォーマットより、常に一定の形状の方が下流の消費側にとってパースしやすい。`format_date_time` は現在 `DateTimeValue` の実フィールド `year`/`month`/`day`/`hour`/`minute`/`second`([model/cell.md オープンクエスチョン4](model/cell.md)、同じくIssue #40で解決)をこの形式へ直接読み込む。
4. **スタイル情報のJSON出力**: さらに解決が進んだ——`JsonCell.style.font`(Issue #38)、`JsonCell.style.wrapText`(Issue #37)、`JsonCell.style.numberFormat`(Issue #41)、`JsonCell.style.alignment`(Issue #42)、`JsonCell.style.fillFgColor`/`fillBgColor`(Issue #75)、`JsonCell.style.borders`(Issue #97)をいずれも上記の通り実装した。[model/style.md オープンクエスチョン1](model/style.md) で追跡されていたサブIssueに加え、派生の塗りつぶし色・罫線Issueも解決済み。`fillFgColor`/`fillBgColor`は生の指定のまま(`rgb`/`theme`/`indexed`のタグ付き、最終的な表示色ではない)保持する——実RGB値への解決はIssue #76であり、本ファイルのスコープ外。`borders`は`model::style::Borders`自身のスコープに合わせ有無のみを報告する(線種/太さ/色は対象外)。
5. ~~一括構築によるピークメモリの抑制~~ → **解決**: `Vec<JsonCell>` を事前構築せず、`Sheet::iter_cells` から得たイテレータを `CellSeq::serialize` 内で直接 `serde::ser::SerializeSeq` へ流し込むストリーミング設計に変更した（[PR #10 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/10#pullrequestreview-4949223332)を反映）。ただし `to_json_string`（内部で `Vec<u8>` バッファを使う簡易版）自体は出力サイズに比例したO(n)のメモリを要する点は変わらない。真にO(1)の追加メモリで完結させたい呼び出し元は `to_json_writer` に `BufWriter<File>` 等の実際のI/O先を渡す必要がある。また `Sheet::iter_cells` が `ExactSizeIterator` を保証しないため `serialize_seq` の要素数ヒントを `None` としている点（JSON出力自体の正しさには影響しないが、一部のシリアライザ実装で軽微な最適化機会を逃す）は、[model/sheet.md](model/sheet.md) 側で `ExactSizeIterator` を公開APIとして約束するかどうかの検討課題として残る。
6. **`to_json_writer`/`to_json_string` と `lib.rs` の公開APIとの関係**: `Workbook` を返す `parse_workbook` とは別に、本関数群を `lib.rs` がどう公開するかは [pipeline.md オープンクエスチョン1](pipeline.md) と連動し、`lib.rs` の設計時に確定させる。
