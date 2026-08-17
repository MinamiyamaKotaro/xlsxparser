# `model/sheet.rs` 設計書

*[English](sheet.en.md)*

`src/model/sheet.rs` に対応する設計書。[model/cell.md](cell.md) の `Cell` / `CellRef` を用いて、1シート分のデータを表す疎行列 `Sheet` を定義する。要求仕様書の 3.1（疎行列によるメモリ最適化）・3.2（結合セルの透過的アクセス）を型として実現する中核モジュール。

## 責務・スコープ

- データまたは書式を持つセルのみを `HashMap<CellRef, Cell>` で保持する疎行列 `Sheet` を定義する
- 結合範囲内の仮想セル座標を起点セルへ解決し、`get()` 経由で透過的にアクセスできるようにする（具体的な解決方法は主要な型セクション参照。実装時に発覚したバグにより、当初ドラフトのセル単位エイリアスマップは不採用となった。コードブロック直後の注記参照）
- `cells` / `merged_regions` はクレート内非公開のまま保持し、`insert_cell` / `insert_merge` / `get_mut` という限定されたAPI（`pub(crate)`）経由でのみ変更を許可することで、`max_row`/`max_col` の同期や結合起点セルの補完といった内部不変条件を`Sheet`自身に強制させる
- **含まない責務**: `<mergeCells>` XMLのパース（`parse/worksheet.rs`）、結合範囲とセルデータを突き合わせて `insert_merge` を呼び出す判断ロジックそのもの（`resolve/merge.rs`。本ファイルは呼び出しを受けて安全にマッピングを構築するAPIのみを提供する）

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
    // `start <= end` は呼び出し側（resolve/merge.rs）が保証すべき事前条件であり、
    // 本型自体は強制しない。リリースビルドで u32 が静かにアンダーフローするのを
    // 防ぐため、row_span/col_span では（debugビルドのみ）assertする
    // （実装時に確定。PR #20 レビューを反映）。
    pub fn row_span(&self) -> u32 { debug_assert!(self.start.row <= self.end.row); self.end.row - self.start.row + 1 }
    pub fn col_span(&self) -> u32 { debug_assert!(self.start.col <= self.end.col); self.end.col - self.start.col + 1 }
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
    /// 起点セル座標 -> 結合範囲。仮想座標を起点へ解決する唯一の
    /// 情報源でもある（`resolve_origin` による幾何学的な包含判定。
    /// コードブロック直後の注記でセル単位エイリアスマップから
    /// 置き換えた経緯を説明）。
    merged_regions: HashMap<CellRef, MergedRegion>,
    /// 挿入されたセルのうち最大の行・列番号。セル挿入のたびに
    /// インクリメンタルに更新し、`<dimension>` 要素の値には依存しない。
    pub max_row: u32,
    pub max_col: u32,
}

impl Sheet {
    /// 新規シートを構築する。`cells` / `merged_regions` は空、
    /// `max_row` / `max_col` は0から開始する。`pipeline.rs` が
    /// [`parse/workbook.rs`](../parse/workbook.md) の結果（`name`/`visibility`）
    /// から構築し、[`parse/worksheet.rs`](../parse/worksheet.md) へ渡して
    /// ストリームでセルを挿入させる（pipeline.md 参照。設計時に発見した
    /// 欠落のため追加）。
    pub(crate) fn new(name: String, visibility: SheetVisibility) -> Self {
        Self {
            name,
            visibility,
            cells: HashMap::new(),
            merged_regions: HashMap::new(),
            max_row: 0,
            max_col: 0,
        }
    }

    /// `r` が結合範囲内に収まる場合はその起点座標へ解決し、収まらない
    /// 場合は `r` をそのまま返す。`merged_regions` への線形走査（結合が
    /// 一件も無い場合は走査自体をスキップする、最も一般的なケース）。
    /// 実運用のシートはシートの寸法によらず結合範囲の件数はせいぜい
    /// 数千件程度に収まるため、この方式でも十分軽量である —
    /// `resolve::merge` の重複検証が既に採用している「想定件数が
    /// 小さい前提でのシンプルなO(N)」と同じ判断。
    fn resolve_origin(&self, r: CellRef) -> CellRef {
        if self.merged_regions.is_empty() {
            return r;
        }
        self.merged_regions
            .values()
            .find(|region| {
                r.row >= region.start.row && r.row <= region.end.row
                    && r.col >= region.start.col && r.col <= region.end.col
            })
            .map_or(r, |region| region.start)
    }

    /// 結合セルのエイリアスを解決したうえでセルを取得する。
    /// 起点・仮想いずれの座標を渡しても同じ `Cell` を返す。
    pub fn get(&self, r: CellRef) -> Option<&Cell> {
        let origin = self.resolve_origin(r);
        self.cells.get(&origin)
    }

    /// 結合セルのエイリアスを解決したうえで可変参照を取得する。
    /// resolve/shared_strings.rs, resolve/style.rs がセルの値・スタイルを
    /// 解決済みデータへ書き換える際に用いる。
    pub(crate) fn get_mut(&mut self, r: CellRef) -> Option<&mut Cell> {
        let origin = self.resolve_origin(r);
        self.cells.get_mut(&origin)
    }

    /// セルを挿入し、max_row/max_col を同時に更新する。cells への書き込みは
    /// このメソッド経由のみとし、次元情報の更新漏れを構造的に防ぐ。
    pub(crate) fn insert_cell(&mut self, r: CellRef, cell: Cell) {
        self.max_row = self.max_row.max(r.row);
        self.max_col = self.max_col.max(r.col);
        self.cells.insert(r, cell);
    }

    /// 結合範囲を起点セルキーで `merged_regions` に登録する（範囲内の
    /// 他座標が結合範囲に属するかどうかは、ここで事前計算せず
    /// `resolve_origin` がオンデマンドに幾何学的判定する。コードブロック
    /// 直後の注記参照）。起点セルがまだ `cells` に存在しない場合
    /// （値も書式も持たない結合範囲）は、空セル（`value: None`,
    /// `style: None`）をプレースホルダーとして挿入する。これにより
    /// `iter_cells` が必ず起点セルを拾い、`json.rs` が row_span/col_span
    /// を含む結合情報を取りこぼさないことを保証する。
    /// 終点座標（`region.end`）は `cells` に挿入されない仮想セルのため、
    /// `insert_cell` 経由では `max_row`/`max_col` に反映されない。結合範囲の
    /// 右下が実際の使用範囲の最大値になるケース（例: 実データはA1のみだが
    /// A1:C3として結合されている）を取りこぼさないよう、ここで明示的に
    /// 終点座標を用いて更新する。
    pub(crate) fn insert_merge(&mut self, region: MergedRegion) {
        debug_assert!(region.start.row <= region.end.row);
        debug_assert!(region.start.col <= region.end.col);
        if !self.cells.contains_key(&region.start) {
            self.insert_cell(region.start, Cell { value: None, style: None });
        }
        self.merged_regions.insert(region.start, region);
        self.max_row = self.max_row.max(region.end.row);
        self.max_col = self.max_col.max(region.end.col);
    }

    /// 起点セルが属する結合範囲をO(1)で取得する（json.rsのrow_span/col_span算出用）。
    pub fn merged_region_at(&self, origin: CellRef) -> Option<&MergedRegion> {
        self.merged_regions.get(&origin)
    }

    /// 起点セルのみを走査するイテレータ（JSON生成用）。`resolve_origin` が
    /// 自分自身とは異なる座標へ解決する座標は除外する: `parse/worksheet.rs`
    /// はストリームする `<c>` 要素ごとに `Cell` を挿入するため、結合範囲内で
    /// 後から仮想セルだと判明する座標（罫線のみのスタイルなど）も `cells` に
    /// 含まれうる。よって `cells` が起点セルのみを保持するとは限らない
    /// （実装時に修正。PR #20 レビュー。このフィルタが無いと、そうした
    /// 仮想セルが `json.rs` の出力に起点セルの重複として漏れ出る）。
    pub fn iter_cells(&self) -> impl Iterator<Item = (CellRef, &Cell)> {
        self.cells.iter().filter(|(&r, _)| self.resolve_origin(r) == r).map(|(&r, c)| (r, c))
    }
}
```

**実装時の修正: `merge_aliases` を廃止（ハングするバグ、`resolve/` 実装時に発覚）。** 上記コードブロックは当初、`insert_merge` が範囲内の全 `(row, col)` の組を走査して `merge_aliases: HashMap<CellRef, CellRef>` へエイリアスを1件ずつ登録するドラフトだった——これは O(row_span × col_span) のループである。Excelの実際の最大寸法いっぱいの正当なシート全体結合（`A1:XFD1048576`、約170億セル）に対してはこのコストが無制限に膨れ上がり、実際に `resolve/merge.rs` のテストを書いている最中にハングすることが判明した（実シートの最大寸法から構築した結合範囲で、テストスイートが2分のタイムアウトを大幅に超過した）。修正では `merge_aliases` を完全に廃止し、`get`/`get_mut`/`iter_cells` は代わりに `resolve_origin` の O(N) 幾何学的走査（N はシート上の結合範囲の件数であり、個々の結合範囲の面積ではない）でオンデマンドに所属を判定する。結合が無ければこの走査自体が完全にスキップされる。これにより `get` の計算量はO(1)からO(N)へ後退するが、Nは実運用では小さく保たれる（結合範囲がどれだけ巨大であっても、実際のスプレッドシートが持つ結合範囲の件数自体はせいぜい数千件程度）。一方でハングは完全に解消され、`insert_merge` 自体もO(1)になる。

## 依存関係

- 依存先: [`model/cell.rs`](cell.md)（`Cell`, `CellRef`）
- 依存元: `model::Workbook`（複数シートを保持）、[`pipeline.rs`](../pipeline.md)（`Sheet::new` でシートを構築する）、`resolve/merge.rs`（`insert_merge` を呼び出して結合セルを登録する）、`resolve/shared_strings.rs` / `resolve/style.rs`（`get_mut` を通じてセルの値・スタイルを解決済みデータへ書き換える）、[`json.rs`](../json.md)（`iter_cells` と `merged_region_at` からJSONを組み立てる）、`parse/worksheet.rs`（`insert_cell` でパース結果を挿入する）

`cells` / `merged_regions` フィールド自体は `pub(crate)` にも公開せず完全に非公開のままとし、これらの内部データ構造への書き込みは `insert_cell` / `insert_merge` / `get_mut` の3メソッドのみに限定する。フィールドを直接 `pub(crate)` にする案（初回レビューでの提案）も検討したが、その場合 `max_row`/`max_col` の更新漏れや結合起点セルの補完漏れを各呼び出し元（`resolve/` 配下の複数モジュール）が個別に守る必要があり、不変条件がクレート全体に分散してしまう。メソッド経由に限定することで不変条件を `Sheet` 自身に閉じ込め、呼び出し側は正しさを気にせず利用できる。

## エラー処理方針

- `get()` / `get_mut()` は疎行列の性質上、セルが存在しない（＝空白セル）ことが正常系であるため `Option` を返す。`Result` にはしない。
- 不正な結合範囲（範囲同士の重複、範囲外座標など）の検証は本ファイルの責務外とし、`resolve/merge.rs` 側で `insert_merge` を呼び出す前にエラー（`error.rs` の共通型）として扱う。`insert_merge` 自体は「渡された範囲は妥当である」という前提のもとで動作する。

## テスト方針

- 空白セル（未挿入の座標）に対する `get()` が `None` を返すことの確認（疎行列の基本挙動）
- 結合範囲内の仮想セル座標に対する `get()` が起点セルと同一の `Cell` を返すことの確認
- `MergedRegion::row_span` / `col_span` の境界値テスト（1x1範囲、大きい範囲）
- `merged_region_at` が起点セル座標から対応する `MergedRegion` をO(1)で取得できることの確認（結合範囲を多数持つシートでの動作確認を含む）
- `iter_cells` が起点セルのみを返し、仮想セル座標を含まないことの確認
- **`insert_merge` で仮想座標になる前に `insert_cell` で `cells` に既存エントリがあった座標も、`iter_cells` から正しく除外されることの確認** — `parse/worksheet.rs` が結合範囲内（後に起点でないと判明する座標、例: 罫線のみのスタイル）に `<c>` 要素をストリームするケース（[PR #20 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/20#pullrequestreview-4949786605)で追加した回帰テスト観点。`iter_cells` にこのフィルタが無いと、このセルが起点セルの重複として `json.rs` の出力に漏れ出る）
- **`insert_merge` にシート全体規模の結合範囲（例: Excelの実際の最大寸法 `A1:XFD1048576`）を渡した場合、ハングせずほぼ一定時間で登録が完了することの確認**（上記コードブロック直後で説明した `merge_aliases` 廃止の回帰テスト）
- **どの結合範囲にも属さない座標が自分自身へ解決され、シート上の他の無関係な結合範囲の存在に影響されないことの確認**（`resolve_origin` の幾何学的包含判定に対する正しさの検証）
- `insert_cell` 呼び出しのたびに `max_row` / `max_col` が正しく更新されることの確認（`<dimension>` を信頼せずに算出できることの確認）
- **値も書式も持たない結合範囲に対して `insert_merge` を呼んだ場合、起点セルが空セルとして `cells` に挿入され、`iter_cells` / `merged_region_at` から正しく参照できることの確認**（PR #5 レビューで追加した回帰テスト観点）
- **実データが `A1` のみだが `A1:C3` として結合されているケースで、`insert_merge` 呼び出し後に `max_row == 3` かつ `max_col == 3` となることの確認**（結合範囲の終点がシートの実質的な使用範囲を広げるケースの回帰テスト）

## 未決事項 / オープンクエスチョン

1. ~~シート次元（使用範囲）の管理~~ → **解決**: サードパーティ製ツールが生成した `<dimension>` 要素は不正確・欠落することがあるため信頼しない。セル挿入のたびに `max_row` / `max_col` をインクリメンタルに更新し、`Sheet` の公開フィールドとして O(1) で取得できるようにする（[PR #5 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948235239)を踏まえて確定）。`insert_merge` は起点セルだけでなく結合範囲の終点座標（`region.end`）でも `max_row`/`max_col` を更新する（仮想セルである終点は `cells` に挿入されないため `insert_cell` 経由では反映されず、別途明示的な更新が必要。[再レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948277539)で指摘・修正）。
2. **`cells` のキー型**: `HashMap<CellRef, Cell>` と要求仕様書の例示 `HashMap<(u32, u32), Cell>` のどちらを採用するか。`CellRef` は `Hash` を実装済みのため型としては等価だが、可読性・API一貫性の観点でどちらにするか要確認。
3. **重複／不正な結合範囲の扱い**: 悪意または破損したXLSXが重複する結合範囲を含む場合、`resolve/merge.rs` がどう振る舞うか（エラーで拒否するか、後勝ちで上書きするか）は未決定。本ファイルのAPI（`insert_merge` を複数回呼んだ場合は単純に上書きする実装を想定）は「後勝ち上書き」を前提にしている点に留意。
4. **凍結行/列など`worksheet.xml`のその他メタデータ**: 要求仕様書では明示されていないが、`freezePane` などを将来的に扱う場合、`Sheet` に持たせるか別型に分離するかは未決定（現時点ではスコープ外として型に含めない）。可視性（`visibility`）については解決済み（workbook.md オープンクエスチョン1参照）。
5. ~~非公開フィールドへのクレート内アクセス~~ → **解決**: `cells` 等のフィールドを直接 `pub(crate)` にするのではなく、`insert_cell` / `insert_merge` / `get_mut` という限定APIを `Sheet` に実装し、それ以外からの直接アクセスを禁止する（[PR #5 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948259819)を踏まえて確定。フィールド直接公開案との比較は依存関係セクション参照）。
