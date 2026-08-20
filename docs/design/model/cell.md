# `model/cell.rs` 設計書

*[English](cell.en.md)*

`src/model/cell.rs` に対応する設計書。[architecture.md](../architecture.md) が定義する `model/` の方針（XMLパースや解決ロジックに依存しない純粋なRustデータ構造）に基づき、セル1つ分の値・参照を表す最も基礎的な型を定義する。`model/sheet.rs` や `model/workbook.rs` はこのファイルの型に依存する。

## 責務・スコープ

- 1セル分のデータ（`Cell`）と、その値のバリアント（`CellValue`）を定義する
- セル座標（行・列）と Excel の A1形式文字列（例: `"B12"`）を相互変換する `CellRef` を定義する
- **含まない責務**: XMLからのパース（`parse/worksheet.rs`）、共有文字列・スタイルの解決処理そのもの（`resolve/`）、結合セルの範囲判定ロジック（`resolve/merge.rs`。`Cell` 自体は自分が結合範囲に属するかを知らない）

## 主要な型（案）

```rust
use std::sync::Arc;

/// 日付・時刻の解決済み値。`resolve/style.rs::serial_to_date_time` が
/// Excelのシリアル値から各暦フィールドへ分解して構築する(Issue #40。
/// オープンクエスチョン4参照)。軽量な `Copy` 値型——ミリ秒以下の精度は
/// 持たない。Excelのシリアル値(`f64`)自体が浮動小数点誤差のノイズを
/// 超える精度を確実に表現できず、また下流ユースケースからも要求されて
/// いないため。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DateTimeValue {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

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
    /// t属性省略時のデフォルト。日付以外のシリアル値はここに含む。
    Number(f64),
    /// numFmt が日付/時刻書式であると resolve/style.rs が判定した場合に
    /// Number から変換される。具体的な型は未決定（オープンクエスチョン4参照）。
    DateTime(DateTimeValue),
    /// 解決済みの文字列（共有文字列 t="s" / インラインstr / str のいずれも解決後はこの形に統一する）。
    /// `Arc<str>` により、同一文字列を指す複数セル間でのアロケーション重複を避ける。
    Text(Arc<str>),
    Boolean(bool),
    /// t="e"。エラーコード文字列（例: "#DIV/0!"）をそのまま保持する。
    Error(String),
}

/// 疎行列上の1エントリ。データまたは書式を持つセルのみが `Sheet` 上に存在する
/// （空白セルはインスタンス化しない、要求仕様書 3.1）。
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// 値を持たない（=書式のみ設定された）セルを表現するため Option とする。
    pub value: Option<CellValue>,
    /// `None` はデフォルトスタイル（未指定）を表す。`Arc` により同一スタイルの
    /// 重複コピーを避け、`StyleSheet` 本体の生存期間から切り離す（詳細は依存関係参照）。
    pub style: Option<Arc<ResolvedStyle>>,
}
```

`ResolvedStyle` は [`model/style.rs`](style.md) が定義する型（PR #8 レビュー指摘を反映し配置を確定）であり、本ファイルはその存在のみを仮定して使用する。`DateTimeValue` は本ファイル内で定義する（未決事項4参照）。

## 依存関係

- 依存先: なし（`model/` 内の兄弟モジュールにも依存しない、リーフモジュール）
- 依存元: `model::Sheet`（`BTreeMap<(u32, u32), Cell>` のキーに `CellRef`相当のタプル、または `CellRef` 自体を使う。`HashMap`から`BTreeMap`への変更の経緯は[model/sheet.md](sheet.md)参照）、`resolve/`、`json.rs`

`resolve/style.rs` は [`model/style.rs`](style.md) が定義する `StyleSheet`（`HashMap<StyleId, Arc<ResolvedStyle>>`）から該当スタイルの `Arc` を各セルへ `clone()` して割り当てる。各セルが `Arc` を通じて実データの所有権（の一部）を持つため、`StyleSheet` コンテナ自体はフェーズ4完了時に破棄でき、`pipeline.rs` が定める即時破棄の方針と、値のコピーを避けたいというメモリ効率要件を両立できる。`resolve/shared_strings.rs` と `Arc<str>` の関係も同様。

## エラー処理方針

- `CellRef::from_a1` は不正な入力（例: `"1A"`, 空文字列, 列オーバーフロー）に対し `panic` せず `Result` を返す。パース起点の入力はすべて外部ファイル（信頼できないXML）由来のため、`error.rs` に定義予定の共通エラー型を用いる。実装時は `"A10000000000000"` のような桁溢れ入力についても `u32` へのパースで確実にオーバーフローを検知し `Err` を返すこと（`panic` させない）。
- `CellValue::Error` はOOXML上のエラーコードをそのまま透過するのみで、パーサー内部では解釈・分岐しない（呼び出し側の責務）。

## テスト方針

- `CellRef::from_a1` / `to_a1` のラウンドトリップテスト（`"A1"`, `"Z1"`, `"AA1"`, `"XFD1048576"`（Excel最大列/行）など境界値を含む）
- 不正なA1文字列（小文字, 記号混入, 列名のみ, 行番号のみ, 桁溢れする行番号）に対する `Err` 返却の確認
- `CellValue` の各バリアントの `PartialEq` 比較テスト
- `value: None`（書式のみ設定されたセル）が正しく保持・比較できることの確認
- 同一スタイル／同一文字列を持つ複数セル間で `Arc::ptr_eq` が真になる（実データが重複コピーされていない）ことの確認

## 未決事項 / オープンクエスチョン

1. ~~`style` フィールドの表現~~ → **解決**: `Option<Arc<ResolvedStyle>>` を採用する（[PR #5 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948235239)を踏まえて確定）。`Arc` により実データの重複コピーを避けつつ、`StyleSheet` コンテナ自体はフェーズ4完了後に破棄できる。
2. ~~共有文字列の表現~~ → **解決**: `CellValue::Text(Arc<str>)` を採用する。理由は上記1と同様。
3. **行・列の桁上限**: Excelの最大列数（16,384列 = XFD）・最大行数（1,048,576行）に対し `u32` で十分だが、`col` を数値として扱うか将来的に列名を別型（`ColumnRef`）として分離するかは未決定。
4. ~~`DateTimeValue` の具体的な型~~ → **解決**(Issue #40): `year`/`month`/`day`/`hour`/`minute`/`second` を単純なフィールドとして持つ軽量な `Copy` 構造体とし、意図的に `chrono` 等の外部日付クレートには依存しない——[`resolve/style.rs`](../resolve/style.md) がExcelのシリアル値を自己完結型の整数演算アルゴリズム(Howard Hinnant の `civil_from_days`、パブリックドメイン)で直接分解して構築し、コンパイル時間・バイナリサイズへの影響をゼロに保つ。Excelの1900年うるう年バグ(シリアル60=架空の「1900-02-29」)と `<workbookPr date1904="1"/>` による代替エポックはいずれも同ファイルで処理する——詳細は [resolve/style.md](../resolve/style.md) 参照。
