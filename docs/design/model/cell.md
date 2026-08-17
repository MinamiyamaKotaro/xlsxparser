# `model/cell.rs` 設計書

`src/model/cell.rs` に対応する設計書。[architecture.md](../architecture.md) が定義する `model/` の方針（XMLパースや解決ロジックに依存しない純粋なRustデータ構造）に基づき、セル1つ分の値・参照を表す最も基礎的な型を定義する。`model/sheet.rs` や `model/workbook.rs` はこのファイルの型に依存する。

## 責務・スコープ

- 1セル分のデータ（`Cell`）と、その値のバリアント（`CellValue`）を定義する
- セル座標（行・列）と Excel の A1形式文字列（例: `"B12"`）を相互変換する `CellRef` を定義する
- **含まない責務**: XMLからのパース（`parse/worksheet.rs`）、共有文字列・スタイルの解決処理そのもの（`resolve/`）、結合セルの範囲判定ロジック（`resolve/merge.rs`。`Cell` 自体は自分が結合範囲に属するかを知らない）

## 主要な型（案）

```rust
/// セル座標。Excelに合わせて1-basedとする（A1 = row:1, col:1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CellRef {
    pub row: u32,
    pub col: u32,
}

impl CellRef {
    /// "A1" 形式の文字列から生成する。
    pub fn from_a1(s: &str) -> Result<Self, crate::error::Error>;

    /// "A1" 形式の文字列へ変換する。
    pub fn to_a1(&self) -> String;
}

/// セルの値。OOXMLの `t` 属性（cell type）に対応するバリアントを持つ。
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    /// t属性省略時のデフォルト。数値・日付はシリアル値としてここに含む。
    Number(f64),
    /// 解決済みの文字列（共有文字列 t="s" / インラインstr / str のいずれも解決後はこの形に統一する）
    Text(String),
    Boolean(bool),
    /// t="e"。エラーコード文字列（例: "#DIV/0!"）をそのまま保持する。
    Error(String),
}

/// 疎行列上の1エントリ。データまたは書式を持つセルのみが `Sheet` 上に存在する
/// （空白セルはインスタンス化しない、要求仕様書 3.1）。
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub value: CellValue,
    pub style: ResolvedStyle,
}
```

`ResolvedStyle` は `model/` 内の別型（`model/mod.rs` もしくは `resolve/style.rs` 側で定義予定）を想定するプレースホルダーで、本ファイルのスコープでは型の存在のみを仮定する。

## 依存関係

- 依存先: なし（`model/` 内の兄弟モジュールにも依存しない、リーフモジュール）
- 依存元: `model::Sheet`（`HashMap<(u32, u32), Cell>` のキーに `CellRef`相当のタプル、または `CellRef` 自体を使う）、`resolve/`、`json.rs`

## エラー処理方針

- `CellRef::from_a1` は不正な入力（例: `"1A"`, 空文字列, 列オーバーフロー）に対し `panic` せず `Result` を返す。パース起点の入力はすべて外部ファイル（信頼できないXML）由来のため、`error.rs` に定義予定の共通エラー型を用いる。
- `CellValue::Error` はOOXML上のエラーコードをそのまま透過するのみで、パーサー内部では解釈・分岐しない（呼び出し側の責務）。

## テスト方針

- `CellRef::from_a1` / `to_a1` のラウンドトリップテスト（`"A1"`, `"Z1"`, `"AA1"`, `"XFD1048576"`（Excel最大列/行）など境界値を含む）
- 不正なA1文字列（小文字, 記号混入, 列名のみ, 行番号のみ）に対する `Err` 返却の確認
- `CellValue` の各バリアントの `PartialEq` 比較テスト

## 未決事項 / オープンクエスチョン

1. **`style` フィールドの表現**: `pipeline.rs` の設計メモ（[architecture.md](../architecture.md#pipelinesrs)）にある通り、`Cell` が解決済みの実データ（`ResolvedStyle` の値そのもの）を持つか、`StyleSheet` 側のインデックス（`StyleId(u32)` 等）を持つかは未決定。前者はスタイル解決後に `StyleSheet` を破棄できる（メモリ効率が良い）が、同一スタイルを持つセルが多い場合に値のコピーが増える。後者は逆にコピーは避けられるが、JSON生成完了まで `StyleSheet` の生存期間を延ばす必要がある。
2. **共有文字列も同様の論点**: `CellValue::Text` を解決済み `String` として持つか、`SharedStringTable` へのインデックスを持つかも上記1と同じトレードオフを持つ。本ファイルでは前者（解決済み）を仮定して記述したが、`resolve/shared_strings.rs` の設計書と合わせて確定させる。
3. **行・列の桁上限**: Excelの最大列数（16,384列 = XFD）・最大行数（1,048,576行）に対し `u32` で十分だが、`col` を数値として扱うか将来的に列名を別型（`ColumnRef`）として分離するかは未決定。
4. **日付の扱い**: OOXMLは日付をシリアル値（`Number`）+ `styles.xml` の numFmt で表現するため、本ファイルの型としては `Number` に含め、日付か否かの判定・変換は `resolve/style.rs` 側の責務とする想定。この分担で問題ないか要確認。
