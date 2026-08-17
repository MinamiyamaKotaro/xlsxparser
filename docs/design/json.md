# `json.rs` 設計書

*[English](json.en.md)*

`src/json.rs` に対応する設計書。[architecture.md](architecture.md) が定義するフェーズ5「JSON生成（返却）」を担う。分析・解決が完了した [`model::Workbook`](model/workbook.md) を、フロントエンド描画に必要な `row_span` / `col_span` などの属性を含むJSONへシリアライズする（要求仕様書5章）。

## 責務・スコープ

- [`model::Workbook`](model/workbook.md) を、シリアライズ専用のJSON DTO（`JsonWorkbook`）へ変換する
- [`Sheet::iter_cells`](model/sheet.md)（起点セルのみを走査）と [`Sheet::merged_region_at`](model/sheet.md) を用いて `row_span` / `col_span` を算出し、結合セルの仮想セル座標をJSON出力へ含めない（要求仕様書3.2、5章の実装）
- [`CellValue`](model/cell.md) の各バリアントに応じて、JSON上の値と種別タグ（`type: "number" | "text" | "boolean" | "error" | "dateTime"`。値を持たないセルは `"empty"`）を出力する
- **含まない責務**: モデルデータの解決・検証そのもの（`resolve/`。本ファイルに到達する時点で `Workbook` は全フェーズの検証を通過済みの正常データのみを保持する）、生成したJSON文字列を実際にファイル・HTTPレスポンス等へ書き出すI/O（呼び出し側の責務）

## 主要な型・関数（案）

```rust
use crate::model::cell::{Cell, CellRef, CellValue};
use crate::model::sheet::{Sheet, SheetVisibility};
use crate::model::workbook::Workbook;
use serde::Serialize;

/// JSON出力専用のシリアライズDTO。`model::Workbook` に直接
/// `#[derive(Serialize)]` しない理由は依存関係セクション参照。
#[derive(Debug, Serialize)]
pub struct JsonWorkbook {
    pub sheets: Vec<JsonSheet>,
}

#[derive(Debug, Serialize)]
pub struct JsonSheet {
    pub name: String,
    pub visibility: &'static str, // "visible" | "hidden" | "veryHidden"
    #[serde(rename = "maxRow")]
    pub max_row: u32,
    #[serde(rename = "maxCol")]
    pub max_col: u32,
    pub cells: Vec<JsonCell>,
}

#[derive(Debug, Serialize)]
pub struct JsonCell {
    pub row: u32,
    pub col: u32,
    pub value: JsonCellValue,
    /// 1（結合されていない）の場合はフィールド自体を省略する。
    #[serde(rename = "rowSpan", skip_serializing_if = "is_one")]
    pub row_span: u32,
    #[serde(rename = "colSpan", skip_serializing_if = "is_one")]
    pub col_span: u32,
    // フォント・塗りつぶし等のスタイル出力は ResolvedStyle の拡張待ち
    // （オープンクエスチョン4参照）。
}

fn is_one(n: &u32) -> bool {
    *n == 1
}

/// 種別タグ付きの値表現。`#[serde(tag = "type", content = "value")]` により
/// `{"type": "number", "value": 42.0}` の形でシリアライズされる（タグ付けの
/// 是非はオープンクエスチョン1参照）。
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum JsonCellValue {
    Number(f64),
    /// 具体的な文字列表現（ISO 8601等）は model::DateTimeValue の型確定後に
    /// 決定する（オープンクエスチョン3参照）。
    DateTime(String),
    Text(std::sync::Arc<str>),
    Boolean(bool),
    Error(String),
    /// 値を持たないセル（書式のみ）。
    Empty,
}

/// `workbook` をJSON DTOへ変換する。到達する時点で `workbook` は全フェーズの
/// 検証を通過済みの正常データのみを保持するため `Result` を返さない
/// （エラー処理方針参照）。
pub fn to_json_workbook(workbook: &Workbook) -> JsonWorkbook {
    JsonWorkbook {
        sheets: workbook.sheets().iter().map(sheet_to_json).collect(),
    }
}

fn sheet_to_json(sheet: &Sheet) -> JsonSheet {
    JsonSheet {
        name: sheet.name.clone(),
        visibility: visibility_tag(sheet.visibility),
        max_row: sheet.max_row,
        max_col: sheet.max_col,
        cells: sheet
            .iter_cells()
            .map(|(cell_ref, cell)| cell_to_json(sheet, cell_ref, cell))
            .collect(),
    }
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
    }
}

fn cell_value_to_json(value: Option<&CellValue>) -> JsonCellValue {
    match value {
        None => JsonCellValue::Empty,
        Some(CellValue::Number(n)) => JsonCellValue::Number(sanitize_float(*n)),
        Some(CellValue::DateTime(dt)) => JsonCellValue::DateTime(format_date_time(dt)),
        Some(CellValue::Text(s)) => JsonCellValue::Text(s.clone()),
        Some(CellValue::Boolean(b)) => JsonCellValue::Boolean(*b),
        Some(CellValue::Error(e)) => JsonCellValue::Error(e.clone()),
    }
}

/// JSONはNaN/Infinityを表現できないため、非有限のf64を0.0へ置き換える
/// フォールバック（妥当な代替値かはオープンクエスチョン2参照）。
fn sanitize_float(n: f64) -> f64 {
    if n.is_finite() { n } else { 0.0 }
}

/// `model::DateTimeValue`（未確定、[model/cell.md オープンクエスチョン4](model/cell.md)参照）
/// を文字列表現へ変換する。具体的なフォーマットは同型の確定と合わせて決定する。
fn format_date_time(dt: &crate::model::cell::DateTimeValue) -> String {
    let _ = dt;
    unimplemented!()
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

- 依存先: [`model/workbook.rs`](model/workbook.md)（`Workbook`）、[`model/sheet.rs`](model/sheet.md)（`Sheet::iter_cells`, `Sheet::merged_region_at`, `SheetVisibility`）、[`model/cell.rs`](model/cell.md)（`Cell`, `CellRef`, `CellValue`, `DateTimeValue`）、外部クレート `serde`（`Serialize` の導出）
- 依存元: `lib.rs`（`Workbook` から明示的に呼び出す。[pipeline.md オープンクエスチョン1](pipeline.md) 参照。`pipeline.rs` の `run` 自体からは呼ばれない）

`model::Workbook` / `Sheet` / `Cell` に直接 `#[derive(Serialize)]` を付与せず、本ファイル専用のDTO（`JsonWorkbook` 等）へ変換してからシリアライズする設計とした。理由は [error.md](error.md) が `Error::XmlParse::source` を型消去して `quick-xml` をパブリック依存にしないようにした設計判断と同じ構造で、`model/` に `serde` への依存を持ち込むと `serde` の破壊的変更が `model/` 側の型定義に波及しうる。DTOへの変換層を挟むことで、`model/` は architecture.md の方針どおり「XMLパースや解決ロジックに依存しない純粋なデータ構造」のまま保たれ、JSON出力の具体的なフィールド名・形状（`camelCase` 化、種別タグの有無など）を `model/` の型定義から独立して変更できる。

## エラー処理方針

- 本ファイルの変換関数は `Result` を返さない。理由: 本ファイルに到達する時点で `model::Workbook` はフェーズ1〜4を通過した妥当なデータのみであり（パース・検証エラーがあれば [`pipeline.rs`](pipeline.md) がその時点で `Err` を返し本ファイルへ到達しない）、本ファイルの変換処理自体が失敗しうる外部要因（信頼できない入力の再解釈など）を持たない
- 唯一JSONが表現できない値である `f64` の `NaN`/`Infinity`（`CellValue::Number` が理論上保持しうる）は、エラーにせず `0.0` へ置き換えるフォールバックとする。[resolve/style.md](resolve/style.md) の `serial_to_date_time` が採用する「個々の値解釈の緩やかな失敗はドキュメント全体を失敗させない」という既存方針を踏襲する（`0.0` が妥当な代替値かはオープンクエスチョン2参照）

## テスト方針

- 単一シート・単一セル（数値）を持つ `Workbook` が `JsonWorkbook` へ正しく変換されることの確認
- 結合セルを持つシートで、起点セルの `rowSpan`/`colSpan` が正しく算出され、仮想セル座標が `cells` 配列に含まれないことの確認（`Sheet::iter_cells` が起点セルのみを返す設計との結線）
- 結合されていない通常セルで `rowSpan`/`colSpan` フィールドがシリアライズ結果から省略される（`skip_serializing_if`）ことの確認
- `CellValue` の各バリアント（`Number`/`Text`/`Boolean`/`Error`）が `JsonCellValue` の対応する `type` タグと `value` で正しくシリアライズされることの確認
- `value: None`（書式のみのセル）が `type: "empty"` としてシリアライズされることの確認
- `CellValue::Number(f64::NAN)` / `CellValue::Number(f64::INFINITY)` を持つセルが `panic` せず `0.0` として出力されることの確認（オープンクエスチョン2の回帰テスト観点）
- 可視性が `Hidden`/`VeryHidden` のシートを含む `Workbook` で、全シートが `visibility` フィールド付きで出力に含まれることの確認（[model/workbook.md](model/workbook.md) の「非表示シートも全て含める」方針との結線）
- 0シートの `Workbook` が `{"sheets": []}` として正しくシリアライズされることの確認

## 未決事項 / オープンクエスチョン

1. **JSON構造における値の種別タグ付けの是非**: 現状 `{"type": "number", "value": 42}` のようなタグ付き表現を採用しているが、フロントエンド側が単純に `value` をそのまま表示する用途であればタグ無しでネイティブなJSON型（数値は `number`、文字列は `string` 等）をそのまま出力する方がシンプルという意見もありうる。ただしその場合 `DateTime` と `Number` をフロントエンド側で区別する手段がなくなるため、どちらを優先するかは要求仕様書のフロントエンド利用シナリオの詳細化と合わせて確定させる。
2. **非有限浮動小数点数（`NaN`/`Infinity`）のフォールバック値**: 現状 `0.0` へ置き換えているが、これは元の値と区別がつかず誤解を招く可能性がある。`null`（`JsonCellValue::Empty` 相当）へフォールバックする、または文字列（`"NaN"` 等）として出力する代替案も検討の余地がある。
3. **`DateTime` の文字列表現形式**: [model/cell.md オープンクエスチョン4](model/cell.md) の `DateTimeValue` 型確定と連動して未確定。ISO 8601（例: `"2024-01-01T00:00:00"`）を軸に検討するが、日付のみ・時刻のみのセルの扱い（Excelは日付/時刻の精度を型として区別しない）は要検討。
4. **スタイル情報のJSON出力**: [model/style.md オープンクエスチョン1](model/style.md) と同一の論点。`ResolvedStyle` が具体的なフォント/塗りつぶし/罫線フィールドを持つまで `JsonCell` にスタイル出力フィールドを追加できない。
5. **一括構築によるピークメモリ使用量**: 要求仕様書が「方眼紙Excel」規模の大規模データを主眼としていることを踏まえると、`Vec<JsonCell>` をシート単位で一括構築してから `serde_json` でシリアライズする現在の設計は、シート1枚が非常に大きい場合にメモリ上のピークサイズが増大する。`serde_json::to_writer` とストリーミングシリアライズ（`SerializeSeq` を手動制御する等）によりセルごとに逐次書き出す設計へ変更する余地があるかは、パフォーマンス要件と合わせて確定させる。
6. **`to_json_workbook` と `lib.rs` の公開APIとの関係**: `Workbook` を返す `parse_workbook` とは別に、本関数（またはこれをラップしたJSON文字列生成関数）を `lib.rs` がどう公開するかは [pipeline.md オープンクエスチョン1](pipeline.md) と連動し、`lib.rs` の設計時に確定させる。
