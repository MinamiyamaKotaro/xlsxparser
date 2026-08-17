# `model/sheet.rs` 設計書

*[English](sheet.en.md)*

`src/model/sheet.rs` に対応する設計書。[model/cell.md](cell.md) の `Cell` / `CellRef` を用いて、1シート分のデータを表す疎行列 `Sheet` を定義する。要求仕様書の 3.1（疎行列によるメモリ最適化）・3.2（結合セルの透過的アクセス）を型として実現する中核モジュール。

## 責務・スコープ

- データまたは書式を持つセルのみを `HashMap<CellRef, Cell>` で保持する疎行列 `Sheet` を定義する
- 結合セルの「仮想セル座標 → 起点セルへのエイリアス参照」マッピングを保持し、`get()` 経由で透過的にアクセスできるようにする
- **含まない責務**: `<mergeCells>` XMLのパース（`parse/worksheet.rs`）、結合範囲とセルデータを突き合わせてエイリアスを構築するロジックそのもの（`resolve/merge.rs`。本ファイルは構築済みマッピングを保持・参照するデータ構造のみを提供する）

## 主要な型（案）

```rust
use std::collections::HashMap;
use crate::model::cell::{Cell, CellRef};

/// 結合範囲。左上（起点セル）と右下の座標を持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergedRegion {
    pub start: CellRef, // 起点セル（実データを保持する）
    pub end: CellRef,
}

impl MergedRegion {
    pub fn row_span(&self) -> u32 { self.end.row - self.start.row + 1 }
    pub fn col_span(&self) -> u32 { self.end.col - self.start.col + 1 }
}

/// シートの可視性（`workbook.xml` の `<sheet state="...">`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetVisibility {
    Visible,
    Hidden,
    VeryHidden,
}

/// 1シート分の疎行列データ。
#[derive(Debug, Clone)]
pub struct Sheet {
    pub name: String,
    pub visibility: SheetVisibility,
    cells: HashMap<CellRef, Cell>,
    /// 仮想セル座標 -> 起点セル座標。resolve/merge.rs が構築する。
    merge_aliases: HashMap<CellRef, CellRef>,
    /// 起点セル座標 -> 結合範囲。キーを起点セルにすることで、
    /// row_span/col_spanの参照をO(1)で行えるようにする。
    merged_regions: HashMap<CellRef, MergedRegion>,
    /// 挿入されたセルのうち最大の行・列番号。セル挿入のたびに
    /// インクリメンタルに更新し、`<dimension>` 要素の値には依存しない。
    pub max_row: u32,
    pub max_col: u32,
}

impl Sheet {
    /// 結合セルのエイリアスを解決したうえでセルを取得する。
    /// 起点・仮想いずれの座標を渡しても同じ `Cell` を返す。
    pub fn get(&self, r: CellRef) -> Option<&Cell> {
        let origin = self.merge_aliases.get(&r).copied().unwrap_or(r);
        self.cells.get(&origin)
    }

    /// 起点セルが属する結合範囲をO(1)で取得する（json.rsのrow_span/col_span算出用）。
    pub fn merged_region_at(&self, origin: CellRef) -> Option<&MergedRegion> {
        self.merged_regions.get(&origin)
    }

    /// 起点セルのみを走査するイテレータ（JSON生成用）。
    pub fn iter_cells(&self) -> impl Iterator<Item = (CellRef, &Cell)>;
}
```

## 依存関係

- 依存先: [`model/cell.rs`](cell.md)（`Cell`, `CellRef`）
- 依存元: `model::Workbook`（複数シートを保持）、`resolve/merge.rs`（`merge_aliases` / `merged_regions` を構築して書き込む）、`resolve/shared_strings.rs` / `resolve/style.rs`（`cells` の値を書き換える）、`json.rs`（`iter_cells` と `merged_region_at` からJSONを組み立てる）、`parse/worksheet.rs` または `resolve/`（セル挿入時に `max_row` / `max_col` を更新する）

## エラー処理方針

- `get()` は疎行列の性質上、セルが存在しない（＝空白セル）ことが正常系であるため `Option` を返す。`Result` にはしない。
- 不正な結合範囲（範囲同士の重複、範囲外座標など）の検証は本ファイルの責務外とし、`resolve/merge.rs` 側でエラー（`error.rs` の共通型）として扱う。`Sheet` はマッピングを構築済みの前提でのみ動作する「信頼された状態」を保持するデータ構造とする。

## テスト方針

- 空白セル（未挿入の座標）に対する `get()` が `None` を返すことの確認（疎行列の基本挙動）
- 結合範囲内の仮想セル座標に対する `get()` が起点セルと同一の `Cell` を返すことの確認
- `MergedRegion::row_span` / `col_span` の境界値テスト（1x1範囲、大きい範囲）
- `merged_region_at` が起点セル座標から対応する `MergedRegion` をO(1)で取得できることの確認（結合範囲を多数持つシートでの動作確認を含む）
- `iter_cells` が起点セルのみを返し、仮想セル座標を含まないことの確認
- セル挿入時に `max_row` / `max_col` が正しく更新されることの確認（`<dimension>` を信頼せずに算出できることの確認）

## 未決事項 / オープンクエスチョン

1. ~~シート次元（使用範囲）の管理~~ → **解決**: サードパーティ製ツールが生成した `<dimension>` 要素は不正確・欠落することがあるため信頼しない。セル挿入のたびに `max_row` / `max_col` をインクリメンタルに更新し、`Sheet` の公開フィールドとして O(1) で取得できるようにする（[PR #5 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948235239)を踏まえて確定）。
2. **`cells` のキー型**: `HashMap<CellRef, Cell>` と要求仕様書の例示 `HashMap<(u32, u32), Cell>` のどちらを採用するか。`CellRef` は `Hash` を実装済みのため型としては等価だが、可読性・API一貫性の観点でどちらにするか要確認。
3. **重複／不正な結合範囲の扱い**: 悪意または破損したXLSXが重複する結合範囲を含む場合、`resolve/merge.rs` がどう振る舞うか（エラーで拒否するか、後勝ちで上書きするか）は未決定。本ファイルのAPI（`merge_aliases` / `merged_regions` を単一の `HashMap` で持つ設計）は「後勝ち上書き」を前提にしている点に留意。
4. **凍結行/列など`worksheet.xml`のその他メタデータ**: 要求仕様書では明示されていないが、`freezePane` などを将来的に扱う場合、`Sheet` に持たせるか別型に分離するかは未決定（現時点ではスコープ外として型に含めない）。可視性（`visibility`）については解決済み（Issue該当なし、workbook.md オープンクエスチョン1参照）。
