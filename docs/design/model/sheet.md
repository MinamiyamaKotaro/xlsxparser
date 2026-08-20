# `model/sheet.rs` 設計書

*[English](sheet.en.md)*

`src/model/sheet.rs` に対応する設計書。[model/cell.md](cell.md) の `Cell` / `CellRef` を用いて、1シート分のデータを表す疎行列 `Sheet` を定義する。要求仕様書の 3.1（疎行列によるメモリ最適化）・3.2（結合セルの透過的アクセス）を型として実現する中核モジュール。

## 責務・スコープ

- データまたは書式を持つセルのみを `BTreeMap<CellRef, Cell>`(Issue #87で`HashMap`から変更。詳細はコードブロック直後の注記参照)で保持する疎行列 `Sheet` を定義する
- 結合範囲内の仮想セル座標を起点セルへ解決し、`get()` 経由で透過的にアクセスできるようにする（具体的な解決方法は主要な型セクション参照。実装時に発覚したバグにより、当初ドラフトのセル単位エイリアスマップは不採用となった。コードブロック直後の注記参照）
- `cells` / `merged_regions` はクレート内非公開のまま保持し、`insert_cell` / `insert_merge` / `get_mut` という限定されたAPI（`pub(crate)`）経由でのみ変更を許可することで、`max_row`/`max_col` の同期や結合起点セルの補完といった内部不変条件を`Sheet`自身に強制させる
- **含まない責務**: `<mergeCells>` XMLのパース（`parse/worksheet.rs`）、結合範囲とセルデータを突き合わせて `insert_merge` を呼び出す判断ロジックそのもの（`resolve/merge.rs`。本ファイルは呼び出しを受けて安全にマッピングを構築するAPIのみを提供する）

## 主要な型（案）

```rust
use std::collections::{BTreeMap, HashMap};
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

/// セルのハイパーリンク(Issue #95)。生のまま、非解決の状態で保持する
/// — `ColorRef`と同じ「表示ではなくdiffのため」という思想(Issue #75)。
/// リンク先の実在確認もHTTPアクセスも一切行わない。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hyperlink {
    pub target: Option<String>,
    pub location: Option<String>,
    pub tooltip: Option<String>,
}

/// ハイパーリンクが適用される範囲(Issue #95)——`start`/`end`は
/// `MergedRegion`と全く同じ形(単一セルの`ref`なら`start == end`)。
/// 理由も同じ「範囲のまま保持し展開しない」: `<hyperlink
/// ref="A1:XFD1048576">`はO(row_span × col_span)ではなくO(1)で
/// 済まなければならない——`insert_merge`が既に塞いだのと同種の増幅
/// (`insert_merge_on_huge_region_does_not_hang`)。`pub(crate)`——
/// `MergedRegion`と異なり利用者向けAPIには一切公開されない。`Sheet`が
/// 公開するのは`hyperlink_at`が返す最終的なセル単位の`Hyperlink`のみ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HyperlinkRange {
    pub start: CellRef,
    pub end: CellRef,
    pub hyperlink: Hyperlink,
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
    cells: BTreeMap<CellRef, Cell>,
    /// 起点セル座標 -> 結合範囲。仮想座標を起点へ解決する唯一の
    /// 情報源でもある（`resolve_origin` による幾何学的な包含判定。
    /// コードブロック直後の注記でセル単位エイリアスマップから
    /// 置き換えた経緯を説明）。
    merged_regions: HashMap<CellRef, MergedRegion>,
    /// 全結合範囲の`start`/`end`を包含する合成バウンディングボックス
    /// （min_row, max_row, min_col, max_col）。結合が無ければ`None`。
    /// `resolve_origin`がこれを先に確認し、範囲外の座標をO(1)で
    /// 早期棄却できるようにする（PR #23 レビュー。コードブロック直後の
    /// 注記参照）。
    merge_bounds: Option<(u32, u32, u32, u32)>,
    /// 挿入されたセルのうち最大の行・列番号。セル挿入のたびに
    /// インクリメンタルに更新し、`<dimension>` 要素の値には依存しない。
    pub max_row: u32,
    pub max_col: u32,
    /// 起点セル座標 -> ハイパーリンク(Issue #95)。`finalize_hyperlinks`が
    /// 一度だけ設定する。`merged_regions`と同じく`HashMap`——出力順序を
    /// 直接左右することはなく、`iter_cells`の(既に決定的な)走査中に
    /// セルごとに引かれるだけである。
    hyperlinks: HashMap<CellRef, Hyperlink>,
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
            cells: BTreeMap::new(),
            merged_regions: HashMap::new(),
            merge_bounds: None,
            max_row: 0,
            max_col: 0,
            hyperlinks: HashMap::new(),
        }
    }

    /// `r` が結合範囲内に収まる場合はその起点座標へ解決し、収まらない
    /// 場合は `r` をそのまま返す。まず `merge_bounds` により全結合範囲の
    /// 合成バウンディングボックス外であればO(1)で棄却し、それ以外の場合は
    /// `merged_regions` への線形走査へフォールバックする。実運用のシートは
    /// シートの寸法によらず結合範囲の件数はせいぜい数千件程度に収まるため、
    /// フォールバック走査自体も十分軽量である —
    /// `resolve::merge` の重複検証が既に採用している「想定件数が
    /// 小さい前提でのシンプルなO(N)」と同じ判断。
    fn resolve_origin(&self, r: CellRef) -> CellRef {
        let Some((min_row, max_row, min_col, max_col)) = self.merge_bounds else {
            return r;
        };
        if r.row < min_row || r.row > max_row || r.col < min_col || r.col > max_col {
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
        let (min_row, max_row, min_col, max_col) = self.merge_bounds.unwrap_or((
            region.start.row, region.end.row, region.start.col, region.end.col,
        ));
        self.merge_bounds = Some((
            min_row.min(region.start.row),
            max_row.max(region.end.row),
            min_col.min(region.start.col),
            max_col.max(region.end.col),
        ));
        self.max_row = self.max_row.max(region.end.row);
        self.max_col = self.max_col.max(region.end.col);
    }

    /// 起点セルが属する結合範囲をO(1)で取得する（json.rsのrow_span/col_span算出用）。
    pub fn merged_region_at(&self, origin: CellRef) -> Option<&MergedRegion> {
        self.merged_regions.get(&origin)
    }

    /// このシートの `<mergeCell>` が全て `insert_merge` で登録された後
    /// （`resolve::merge::resolve` の最後のステップとして呼ばれる）に
    /// 一度だけ実行する。現時点で挿入済みの全セルキーを、行方向の
    /// スイープラインでバッチ解決する——C個のセル・M個の結合範囲に対して
    /// O((C + M) log (C + M))——その上で、自分自身が起点でないエントリを
    /// すべて削除する。詳細と理由（Issue #43）はコードブロック直後の注記、
    /// これが何を実現するかは `iter_cells` のdocコメントを参照。
    pub(crate) fn finalize_merges(&mut self) {
        if self.merged_regions.is_empty() {
            return;
        }

        enum SweepEvent {
            Start(CellRef),        // region.start.row で発火
            End(CellRef),          // region.end.row + 1 で発火
            Query(CellRef),        // セル座標の行で発火
        }

        let mut events: Vec<(u32, u8, SweepEvent)> = Vec::new();
        for region in self.merged_regions.values() {
            events.push((region.start.row, 0, SweepEvent::Start(region.start)));
            events.push((region.end.row + 1, 0, SweepEvent::End(region.start)));
        }
        for &coord in self.cells.keys() {
            events.push((coord.row, 2, SweepEvent::Query(coord)));
        }
        // 同一行では End/Start（rank 0/1）を Query より先に処理し、
        // クエリが常にその行までの最新のアクティブ集合を参照できるようにする。
        events.sort_by_key(|(row, rank, event)| {
            let start_end_rank = match event {
                SweepEvent::End(_) => 0,
                SweepEvent::Start(_) => 1,
                SweepEvent::Query(_) => *rank,
            };
            (*row, start_end_rank)
        });

        // 現在の走査行でアクティブな結合範囲を、各範囲の`start`
        // （merged_regionsのキー）として`start.col`でソートして保持する。
        // 列範囲は構成上排他的（resolve::mergeが重複を拒否するため）なので、
        // クエリ列を含みうるアクティブなエントリは高々1件。
        let mut active: Vec<CellRef> = Vec::new();
        let mut to_drop: Vec<CellRef> = Vec::new();
        for (_, _, event) in &events {
            match event {
                SweepEvent::Start(start) => {
                    let pos = active.partition_point(|s| s.col < start.col);
                    active.insert(pos, *start);
                }
                SweepEvent::End(start) => {
                    let pos = active.partition_point(|s| s.col < start.col);
                    active.remove(pos);
                }
                SweepEvent::Query(coord) => {
                    let pos = active.partition_point(|s| s.col <= coord.col);
                    if pos == 0 {
                        continue;
                    }
                    let candidate = active[pos - 1];
                    if *coord == candidate {
                        continue; // すでにこの結合範囲自身の起点である
                    }
                    let region = self.merged_regions.get(&candidate).unwrap();
                    if coord.col <= region.end.col {
                        to_drop.push(*coord);
                    }
                }
            }
        }
        for coord in to_drop {
            self.cells.remove(&coord);
        }
    }

    /// 起点セルのみを走査するイテレータ（JSON生成用）。もはや
    /// `resolve_origin` を呼ばない: `finalize_merges`（結合範囲が全て
    /// 登録された直後に一度だけ呼ばれる）により、残っている `cells`
    /// のキーは全て自分自身の起点であることが既に保証されているため、
    /// 単純な `cells.iter()` だけで正しい（PR #20時代のフィルタ付き版から
    /// 変更した理由はコードブロック直後の注記(Issue #43)参照）。`cells`が
    /// `BTreeMap`であることにより、この走査順は`CellRef`の導出`Ord`
    /// （行→列の順で比較）に従う行優先・列優先の決定的な順序となる
    /// （Issue #87。詳細はコードブロック直後の注記参照）。
    pub fn iter_cells(&self) -> impl Iterator<Item = (CellRef, &Cell)> {
        self.cells.iter().map(|(&r, c)| (r, c))
    }

    /// `r`にまだ何も無ければ空セルをプレースホルダーとして挿入する
    /// (Issue #95)——`insert_merge`の起点補完と同じ理由。
    fn backfill_blank_cell(&mut self, r: CellRef) {
        if !self.cells.contains_key(&r) {
            self.insert_cell(r, Cell { value: None, style: None });
        }
    }

    /// 検証済みのハイパーリンク範囲をまとめて登録する(重複検証後に
    /// `resolve::hyperlink::resolve`から呼ばれる。resolve/hyperlink.md
    /// 参照)。各範囲の起点セルを補完したうえで、`cells`に既に存在する
    /// 全セルキー(この補完で追加した分を含む)を、それを覆う範囲の
    /// `Hyperlink`へ1回のスイープラインパスで解決する——
    /// `finalize_merges`と同じStart/End/Query構成で、C個のセル・H個の
    /// 範囲に対しO((C + H) log (C + H))、O(C * H)には決してならない。
    /// カバーされたセルが結合の仮想セルのように起点へ畳み込まれず、
    /// 自分自身の座標のままキーになる理由はコードブロック直後の
    /// 注記参照。
    pub(crate) fn finalize_hyperlinks(&mut self, ranges: Vec<HyperlinkRange>) {
        if ranges.is_empty() {
            return;
        }
        for range in &ranges {
            self.backfill_blank_cell(range.start);
        }

        enum SweepEvent {
            Start(usize), // rangesへのインデックス
            End(usize),
            Query(CellRef),
        }

        let mut events: Vec<(u32, u8, SweepEvent)> = Vec::new();
        for (i, range) in ranges.iter().enumerate() {
            events.push((range.start.row, 0, SweepEvent::Start(i)));
            events.push((range.end.row + 1, 0, SweepEvent::End(i)));
        }
        for &coord in self.cells.keys() {
            events.push((coord.row, 2, SweepEvent::Query(coord)));
        }
        events.sort_by_key(|(row, kind_rank, event)| {
            let start_end_rank = match event {
                SweepEvent::End(_) => 0,
                SweepEvent::Start(_) => 1,
                SweepEvent::Query(_) => *kind_rank,
            };
            (*row, start_end_rank)
        });

        let mut active: Vec<usize> = Vec::new(); // rangesへのインデックス、start.col順
        for (_, _, event) in &events {
            match event {
                SweepEvent::Start(i) => {
                    let col = ranges[*i].start.col;
                    let pos = active.partition_point(|&j| ranges[j].start.col < col);
                    active.insert(pos, *i);
                }
                SweepEvent::End(i) => {
                    let col = ranges[*i].start.col;
                    let pos = active.partition_point(|&j| ranges[j].start.col < col);
                    active.remove(pos);
                }
                SweepEvent::Query(coord) => {
                    let pos = active.partition_point(|&j| ranges[j].start.col <= coord.col);
                    if pos == 0 {
                        continue;
                    }
                    let candidate = active[pos - 1];
                    let range = &ranges[candidate];
                    if coord.col <= range.end.col {
                        self.hyperlinks.insert(*coord, range.hyperlink.clone());
                    }
                }
            }
        }
    }

    /// セル`origin`に登録されたハイパーリンクをO(1)で取得する。
    /// `merged_region_at`の慣習をそのまま踏襲する(結合起点への解決は
    /// ここでは行わない——`json.rs`は常に`iter_cells`が返した座標で
    /// これを呼ぶ)。
    pub fn hyperlink_at(&self, origin: CellRef) -> Option<&Hyperlink> {
        self.hyperlinks.get(&origin)
    }
}
```

**実装時の修正: `merge_aliases` を廃止（ハングするバグ、`resolve/` 実装時に発覚）。** 上記コードブロックは当初、`insert_merge` が範囲内の全 `(row, col)` の組を走査して `merge_aliases: HashMap<CellRef, CellRef>` へエイリアスを1件ずつ登録するドラフトだった——これは O(row_span × col_span) のループである。Excelの実際の最大寸法いっぱいの正当なシート全体結合（`A1:XFD1048576`、約170億セル）に対してはこのコストが無制限に膨れ上がり、実際に `resolve/merge.rs` のテストを書いている最中にハングすることが判明した（実シートの最大寸法から構築した結合範囲で、テストスイートが2分のタイムアウトを大幅に超過した）。修正では `merge_aliases` を完全に廃止し、`get`/`get_mut`/`iter_cells` は代わりに `resolve_origin` の O(N) 幾何学的走査（N はシート上の結合範囲の件数であり、個々の結合範囲の面積ではない）でオンデマンドに所属を判定する。結合が無ければこの走査自体が完全にスキップされる。これにより `get` の計算量はO(1)からO(N)へ後退するが、Nは実運用では小さく保たれる（結合範囲がどれだけ巨大であっても、実際のスプレッドシートが持つ結合範囲の件数自体はせいぜい数千件程度）。一方でハングは完全に解消され、`insert_merge` 自体もO(1)になる。

**追加の最適化: `merge_bounds`（PR #23 レビュー）。** `Sheet` はさらに `merge_bounds: Option<(u32, u32, u32, u32)>`——全結合範囲の `start`/`end` を包含する合成バウンディングボックス（行・列それぞれの最小値・最大値）——を保持し、`insert_merge` 内で `merged_regions` と合わせて更新する。`resolve_origin` はまずこれを確認し、合成バウンディングボックスの外側にある座標はO(N)の個別範囲走査に入る前にO(1)で棄却する。結合範囲が特定の領域に集中しているシートでは、大半のセルがその領域の外側にあるため、この事前チェックにより一般的なケースを実質O(1)に戻せる。一方で、バウンディングボックス内だが個々のどの範囲にも属さない座標（2つの結合範囲の間の隙間など）については、O(N)のフォールバック走査が引き続き正しく機能することを回帰テスト `get_inside_bounding_box_but_outside_any_region_resolves_to_itself` で確認している。このバウンディングボックスは常に最もタイトな値とは限らない保守的な上限である点に注意: 同じ起点セルへの `insert_merge` の上書きでより小さい範囲に置き換わっても、古い（より大きい）境界は縮小されない。これはまれなケースでO(1)早期棄却の機会を1回逃すだけであり、正当性には影響しない——バウンディングボックスチェックが座標を棄却しない限り、最終的な判定は常にO(N)の全走査が担うため。

**修正: `finalize_merges`（Issue #43。`merge_bounds` では塞ぎきれなかった、意図的な結合配置による抜け道の解消）。** `merge_bounds` のO(1)事前チェックは、合成バウンディングボックスの**外側**にある座標しか棄却できない。対角に1x1の結合セルを2個置くだけの、ごく小さな正当な配置でこのボックスはシート全体近くまで拡大しうるため、それ以外の全セルは実際の結合セルからの距離に関係なくO(N)の個別範囲走査にフォールバックしてしまう。`json.rs` の `iter_cells` はセルごとに `resolve_origin` を呼ぶため、これがJSON生成時にO(セル数 × 結合範囲数)のコストへと転化していた——既存の全ての上限内(数十万セル、`MAX_MERGE_REGIONS` 以内の数万件の結合セル、ディスク上数百KB)に収まる正当なファイルで、実測で数十秒のCPU時間になることを確認した。

3種類の「賢い」個別クエリ向け対策を試したが、いずれも特定の(だが完全に正当な)結合配置により元とほぼ同等のコストへ劣化することが実測で判明した: グローバルな「これまで見た最大の結合高さ」による打ち切り(全高の結合セルを1個追加するだけで無効化される)、固定サイズの行バケット分割(結合セルとクエリを1バケットに集中させるだけで無効化される)、両方の子を探索してしまう高さ平衡区間木(行範囲は広いが列が異なる結合セル群で無効化される)。各反例と実測値はIssue #43の議論スレッド参照。

最終的に有効だったのが `Sheet::finalize_merges` である。シートの全結合セルが `insert_merge` で登録された直後に `resolve::merge::resolve` から一度だけ呼ばれる。現時点で挿入済みの全セルキーを、行方向の単一スイープラインパスで起点へ解決する——各結合範囲の行範囲からStart/Endイベントを生成し、セルキーごとのQueryイベントと合わせて1回だけソートし(C個のセル・M個の結合範囲に対してO((C + M) log (C + M)))、その行で現在アクティブな結合セル集合(`resolve::merge` の検証により重複しないことが構成上保証されている、列でソートされた集合)を維持しながら1回だけ走査する——その上で、自分自身が起点でないセルを全て削除する。これは観測可能なデータを一切失わない: 自分自身が起点でない座標はもともと `get`/`iter_cells` からは到達不能だった(`resolve_origin` が常に先に起点へリダイレクトするため)ので、削除されるエントリは元々死んでいたものであり、これは既存の回帰テスト `iter_cells_excludes_cells_pre_inserted_at_alias_coordinates` が既に確認している事実そのものである。変わるのは、`iter_cells` がその後 `resolve_origin` を一切呼ばなくて済むようになる点だけである——残っている全てのキーが既に自分自身の起点と等しいため——これにより、結合セルが空間的にどう配置されていてもコスト経路が塞がれる。`get`/`get_mut` 自身の汎用フォールバック(パース完了後に外部呼び出し元が任意の座標を問い合わせる用途)は変更しない。攻撃者が実際に制御できる、内部駆動でファイルサイズに比例する `iter_cells` の経路だけがこの対応を必要としていた。

**機能: 列幅(Issue #39)。** `Sheet` は `col_widths: Vec<ColWidthRange>`（`min` でソート済み・相互に非重複）と `default_col_width: Option<f64>` も保持し、`resolve::column_width::resolve` が `Sheet::set_col_widths` を通じて一度だけ登録する——`resolve::merge`/`insert_merge` と同じ「検証してから登録し、事前条件を信頼する」という分担。`ColWidthRange { min, max, width }` は `MergedRegion` の「範囲のまま保持し展開しない」という原則を踏襲しており、実データに頻出する単一の `<col min="1" max="16384" .../>` は16,384件ではなく1件として登録されなければならない(`resolve::column_width` の回帰テスト `a_full_width_single_range_does_not_expand_into_per_column_entries` 参照)。

`column_width(col) -> Option<f64>` は `col_widths` を二分探索する——`partition_point` で `min <= col` を満たす最後の範囲を求め、その範囲の `max` が実際に `col` まで届くか確認する——ファイルが範囲をどう配置してもO(log R)になる(RはRの上限 `resolve::column_width::MAX_COLUMN_WIDTH_RANGES` = 2,000でキャップ)。`col` を覆う範囲も `defaultColWidth` も無い場合はExcelの一般的な既定値(「Calibri 11 ≈ 8.43文字」など)を推測で埋めるのではなく `None` を返す: そのフォールバックはこのライブラリが計算していないフォントメトリクスに依存するため、誤った数値より明示的な不在の方が望ましい。`col_width_ranges()` は生のソート済み `Vec` を公開し、`json.rs` がシート単位の `columns` 配列としてシリアライズする——意図的に**セルごとに引いてセルのJSONオブジェクトへ埋め込まない**: 列単位の値をその列の全ての実在セルに繰り返すと、このライブラリの存在意義である疎な出力設計に何の利益も無く反する(列幅専用のサブIssueができる前、Issue #36のレビュー議論で提起された)。

**機能: 画像(Issue #65)。** `Sheet` は `images: Vec<Image>` も保持し、`pipeline.rs` のフェーズ3.5が `Sheet::set_images` を通じて一度だけ登録する——`set_col_widths` と同じ「他所で解決し、一度だけ登録する」という分担。`merged_regions` と異なり、画像はいかなるセル座標にも紐付けられない: `ImageAnchor::TwoCell`/`OneCell` のマーカーはセル内のEMU単位オフセットを持つため、アンカーの位置は `MergedRegion` のように常にセル境界に一致するとは限らず、画像が自然に「所属する」単一のセルというものが存在しない。`images()` は生の `Vec` を公開し、`json.rs` がシート単位の `images` 配列としてシリアライズする——`col_width_ranges`(上記)と同じ疎な出力設計の理由に加え、セルごとの複製が望ましい場合であってもそもそも画像を紐付けるセルが存在しないという点が加わる。

**修正: `cells` を `HashMap` から `BTreeMap` へ変更（Issue #87）。** `iter_cells` の走査順は `json.rs` の `cells` 配列にそのまま反映されるが、`HashMap` の走査順はプロセスごとにランダムなハッシュシード（HashDoS対策）に依存するため、同一ファイルを2回パースしても出力されるJSONのセル順が毎回変わりうる。座標で突き合わせる用途（`(row, col)`をキーにした差分）には無害だが、2回のパース結果をテキスト差分（`git diff`等）で比較する用途では、実際には変更が無いセルが並び替わっただけで大量の差分として現れてしまう問題が報告された。`BTreeMap` はキー型`CellRef`の導出`Ord`（フィールド宣言順、つまり`row`を`col`より先に比較）に従って走査されるため、追加のソート処理を挟むことなく、人が読んで自然な行優先・列優先の順序を無条件に保証できる。

Issue #87のPoC（`massive_dense_accounting.xlsx`、30万セル、カスタムのバイトカウント方式アロケータおよびmacOSの`sample`プロファイラで実測）による検証結果:

- **CPU/実時間**: `BTreeMap`の方が約9〜13%高速（`HashMap`が定評として持つ「ハッシュテーブルの方が速い」という直感に反する結果だが、`CellRef`が8バイトの小さいキーであるため、SipHashの計算コストがB-treeノード内の数回の整数比較のコストを上回ることが実測で判明した）。
- **パース中のピークメモリ**: `BTreeMap`の方が約26%良好。`HashMap::new()`は無容量から開始し、ストリームでセルを挿入する過程で再ハッシュ・再配置を繰り返すたびに新旧2つのテーブルを一時的に同時保持するため、定常状態に対して約54%のピークスパイクが生じる。`BTreeMap`はノード単位でインクリメンタルに成長するため、このスパイクが発生しない。
- **定常メモリ**: `BTreeMap`の方が約9.2%悪化（1セルあたり約78.3バイト vs `HashMap`の約71.7バイト、ノード/ポインタのオーバーヘッド）。決定的な出力順を得るためのトレードオフとして許容した。挿入順（昇順/降順/シャッフル）による定常メモリへの実測上の有意差は無いことも別途のPoCで確認済み（`Sheet::insert_cell`は`parse/worksheet.rs`がXML出現順に呼ぶため、行・列が昇順とは限らない実ファイルでも定常メモリの見積もりは変わらない）。

`merged_regions`（起点セル座標をキーとする内部専用のO(1)/O(N)ルックアップ用途で、`json.rs`へ直接反映される走査順を持たない）は本対応のスコープ外として`HashMap`のまま据え置いている。

**機能: ハイパーリンク(Issue #95)。** `Sheet` は `hyperlinks: HashMap<CellRef, Hyperlink>` も保持し、`finalize_hyperlinks` が一度だけ設定する——`resolve::hyperlink::resolve`(重複と開始・終了座標の大小関係を検証済みの`HyperlinkRange`バッチを渡す。[resolve/hyperlink.md](../resolve/hyperlink.md)参照)から呼ばれる。`<hyperlink ref="A1:C3">` は(`<mergeCell>`と異なり)必ずしも結合範囲ではない——OOXMLでは、互いに独立した複数セルの矩形選択に対して1つのハイパーリンクを適用できるため、カバーされた各セルは結合の仮想セルのように起点へ畳み込まれるのではなく、それぞれ独立してJSON出力上でハイパーリンクを持たなければならない。

これにより、`resolve_origin`のパターン(仮想座標を1つの共有起点`Cell`へ解決する`get`/`get_mut`方式)をそのまま流用する案は却下された: 最初のドラフトはまさにそれ(幾何学的バウンディングボックスによる事前チェック→クエリごとに範囲リストを線形`.find()`)を行っており、これは`resolve_origin`の**Issue #43修正前**の形そのものであり、`finalize_merges`のスイープライン書き換えが結合セルについて解消したはずのO(セル数 × 範囲数)のコストを再導入してしまうことが設計レビューの段階(実装前)で判明した(この再導入を防ぐ回帰テストについては`resolve/hyperlink.md`のテスト方針参照)。`finalize_hyperlinks`は代わりに同じスイープを一度だけ実行するが、一致した際に**クエリ**座標(カバーされた各セルそれぞれ)をキーとして`hyperlinks`へ直接挿入する——`finalize_merges`のように非起点キーを削除するのではない。起点セル自身も、スイープが走る前に`backfill_blank_cell`がその存在を既に保証しているため、同じパスの中で自然に自分自身のエントリを得る。(最初の実装は一致結果を中間`Vec`にバッファし、その後の2回目のループで`hyperlinks`へ挿入していた——スイープ中に`self`を変更しないという、特に検証していない習慣によるもの。PR #96のCopilot PRレビューが、ループ内のどこも`self.hyperlinks`を他所で借用していない(`ranges`/`active`/`events`はいずれもスイープ内ローカル)ことを指摘したため、挿入を`Query`アーム内へ直接移し、バッファと2回目のループの両方を削除した。)

補完(バックフィル)するのは範囲自身の起点セルのみである(`insert_merge`をそのまま踏襲)——範囲内の他のセルで、値・スタイル・ハイパーリンク以外の理由で存在すべき根拠が無いものは実体化されないままとなり、`iter_cells`/JSON出力からは見えない(Excel上ではクリック可能に表示されるにもかかわらず)。範囲内の全セルを補完する案も検討したが却下した——正当に`MAX_HYPERLINKS_PER_SHEET`規模まで許容される範囲がどれだけ大きくなり得るかに上限が無いため、`insert_merge`から`merge_aliases`を廃止した際(上記の注記参照)に結合セルについて塞いだのと同じO(row_span × col_span)の増幅を再び開けてしまう。これは既知の制限として受け入れており(`resolve/hyperlink.md`のオープンクエスチョン参照)、投機的に解決はしていない。

`Sheet::hyperlink_at`は`get`と異なり結合起点への解決を行わない——`merged_region_at`の慣習をそのまま踏襲する(`json.rs`はいずれも`iter_cells`が既に返した座標——構成上常に起点である——でしか呼ばない)。

## 依存関係

- 依存先: [`model/cell.rs`](cell.md)（`Cell`, `CellRef`）
- 依存元: `model::Workbook`（複数シートを保持）、[`pipeline.rs`](../pipeline.md)（`Sheet::new` でシートを構築する。フェーズ3.5が `set_images` を呼ぶ——[parse/drawing.md](../parse/drawing.md) 参照）、`resolve/merge.rs`（`insert_merge` を呼び出して結合セルを登録し、全件登録後に `finalize_merges` を呼ぶ）、`resolve/shared_strings.rs` / `resolve/style.rs`（`get_mut` を通じてセルの値・スタイルを解決済みデータへ書き換える）、`resolve/column_width.rs`（検証後に `set_col_widths` を呼ぶ）、[`resolve/hyperlink.rs`](../resolve/hyperlink.md)（検証後に一度だけ`finalize_hyperlinks`を呼ぶ）、[`json.rs`](../json.md)（`iter_cells`・`merged_region_at`・`col_width_ranges`・`default_col_width`・`images`・`hyperlink_at` からJSONを組み立てる）、`parse/worksheet.rs`（`insert_cell` でパース結果を挿入する）

`cells` / `merged_regions` フィールド自体は `pub(crate)` にも公開せず完全に非公開のままとし、これらの内部データ構造への書き込みは `insert_cell` / `insert_merge` / `get_mut` / `finalize_merges` に限定する。フィールドを直接 `pub(crate)` にする案（初回レビューでの提案）も検討したが、その場合 `max_row`/`max_col` の更新漏れや結合起点セルの補完漏れを各呼び出し元（`resolve/` 配下の複数モジュール）が個別に守る必要があり、不変条件がクレート全体に分散してしまう。メソッド経由に限定することで不変条件を `Sheet` 自身に閉じ込め、呼び出し側は正しさを気にせず利用できる。

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
- **合成バウンディングボックス `merge_bounds` の内側だが個々のどの結合範囲にも属さない座標（2つの結合範囲の間の隙間）が正しく自分自身へ解決されることの確認**（PR #23 レビューで追加した `merge_bounds` のO(1)事前チェックに特化した正当性テスト。バウンディングボックスが座標を棄却しなかった場合でも、最終判定を担うO(N)の全走査を迂回してはならない）
- `insert_cell` 呼び出しのたびに `max_row` / `max_col` が正しく更新されることの確認（`<dimension>` を信頼せずに算出できることの確認）
- **値も書式も持たない結合範囲に対して `insert_merge` を呼んだ場合、起点セルが空セルとして `cells` に挿入され、`iter_cells` / `merged_region_at` から正しく参照できることの確認**（PR #5 レビューで追加した回帰テスト観点）
- **実データが `A1` のみだが `A1:C3` として結合されているケースで、`insert_merge` 呼び出し後に `max_row == 3` かつ `max_col == 3` となることの確認**（結合範囲の終点がシートの実質的な使用範囲を広げるケースの回帰テスト）
- **結合が無いシートで `finalize_merges` がno-opであることの確認**（共通ケースを軽量に保つ必要がある）
- **`finalize_merges` が、仮想（非起点）座標に事前挿入されたセルを削除しつつ、結合の起点セルと無関係な独立セルは保持することの確認**（Issue #43。これにより `iter_cells` 自身がフィルタを持つ必要がなくなる）
- **`finalize_merges` が、行範囲の重ならない複数の結合範囲にまたがって正しく解決できることの確認**（単一範囲だけでなく、どの範囲にも属さないセルも含めてスイープラインのStart/End管理を検証する）
- **エンドツーエンド回帰テスト: `MAX_MERGE_REGIONS` 件の結合セルを`merge_bounds`が最大化するよう配置(対角に2個配置)し、さらに無関係なセルを数十万件加えたファイルが、修正前に実測した数秒単位の停止なしにJSON生成を完了できることの確認**（`tests/security.rs` の `sparse_merge_bounding_box_does_not_amplify_json_generation_cost`、`sparse_merge_bounding_box_amplification` フィクスチャを使用。意図的な配置によるDoS懸念であるため、`zip_bomb`/`zip_slip`/`xxe_attack` と同じくCategory 4（負荷）ではなくCategory 5（セキュリティ）に分類）
- **`column_width` が範囲なし・`defaultColWidth` なしで `None` を返すことの確認**、**複数範囲にまたがる二分探索の正当性の確認**（範囲内・範囲間の隙間・`defaultColWidth`へのフォールバックの境界値を含む）、**`col_width_ranges`/`default_col_width` がJSON出力用に生の値を公開することの確認**（Issue #39。詳細な検証は `resolve::column_width` のテスト群が担う）
- **`images()` が `set_images` で設定された生の `Vec` をそのまま公開することの確認**（Issue #65。アンカーごとの解決の正当性は `parse::drawing` と `pipeline.rs` それぞれのテスト群が担う）
- **`iter_cells` の走査順が、挿入順によらず行優先・列優先の決定的な順序（`CellRef`の`Ord`に従う）になることの確認**（Issue #87。`cells`を`BTreeMap`へ変更したことによる保証。XML出現順が昇順とは限らない実ファイルを模した回帰テストとして、[`tests/normal.rs`の`json_cells_array_is_sorted_by_row_then_col_regardless_of_source_order`](../../../tests/normal.rs)が挿入順をわざと入れ替えたフィクスチャで検証する）
- **範囲リストが空の場合、`finalize_hyperlinks` がno-opであることの確認**（Issue #95。共通ケースを軽量に保つ必要がある。`finalize_merges`と同型）
- **`finalize_hyperlinks` が範囲の起点セルに空セルをプレースホルダーとして補完し、`hyperlink_at`/`iter_cells` から正しく取得できることの確認**（`insert_merge_backfills_blank_origin_cell`と同型）
- **既にデータを持つ複数セルにまたがるハイパーリンク範囲(結合ではない)が、起点だけでなくカバーする全セルに独立してハイパーリンクを付与することの確認**——`resolve_origin`方式の解決では(Issue #43修正前のコストを再導入せずには)満たせなかった中核的な正当性要件(上記の「機能: ハイパーリンク」の注記参照)
- **ハイパーリンク範囲内の、起点以外の完全に空白なセルが `iter_cells`/JSON出力に現れないことの確認**——上述の受け入れた制限を固定するテストであり、将来これを補完する変更が意図的な決定であって偶発的な退行でないことを保証する
- **結合と同型のエンドツーエンド回帰テスト**: `MAX_HYPERLINKS_PER_SHEET`件の範囲を同時アクティブ行数が最大になるよう配置し、さらに無関係なセルを多数加えたシートが、セル数×範囲数に比例しないコストで完了することの確認(本モジュール単体ではなく`resolve::hyperlink`自身のテストスイートが`pipeline.rs`経由で担う。[resolve/hyperlink.md](../resolve/hyperlink.md)参照)

## 未決事項 / オープンクエスチョン

1. ~~シート次元（使用範囲）の管理~~ → **解決**: サードパーティ製ツールが生成した `<dimension>` 要素は不正確・欠落することがあるため信頼しない。セル挿入のたびに `max_row` / `max_col` をインクリメンタルに更新し、`Sheet` の公開フィールドとして O(1) で取得できるようにする（[PR #5 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948235239)を踏まえて確定）。`insert_merge` は起点セルだけでなく結合範囲の終点座標（`region.end`）でも `max_row`/`max_col` を更新する（仮想セルである終点は `cells` に挿入されないため `insert_cell` 経由では反映されず、別途明示的な更新が必要。[再レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948277539)で指摘・修正）。
2. **`cells` のキー型**: `BTreeMap<CellRef, Cell>` と要求仕様書の例示 `HashMap<(u32, u32), Cell>` のどちらを採用するか。`CellRef` は `Hash`/`Ord` いずれも実装済みのため型としては等価だが、可読性・API一貫性の観点でどちらにするか要確認（コンテナ型自体の選定は項目6で解決済み。本項目はキーを`CellRef`構造体のままにするかタプルに崩すかという別軸の論点）。
3. **重複／不正な結合範囲の扱い**: 悪意または破損したXLSXが重複する結合範囲を含む場合、`resolve/merge.rs` がどう振る舞うか（エラーで拒否するか、後勝ちで上書きするか）は未決定。本ファイルのAPI（`insert_merge` を複数回呼んだ場合は単純に上書きする実装を想定）は「後勝ち上書き」を前提にしている点に留意。
4. **凍結行/列など`worksheet.xml`のその他メタデータ**: 要求仕様書では明示されていないが、`freezePane` などを将来的に扱う場合、`Sheet` に持たせるか別型に分離するかは未決定（現時点ではスコープ外として型に含めない）。可視性（`visibility`）については解決済み（workbook.md オープンクエスチョン1参照）。
5. ~~非公開フィールドへのクレート内アクセス~~ → **解決**: `cells` 等のフィールドを直接 `pub(crate)` にするのではなく、`insert_cell` / `insert_merge` / `get_mut` という限定APIを `Sheet` に実装し、それ以外からの直接アクセスを禁止する（[PR #5 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948259819)を踏まえて確定。フィールド直接公開案との比較は依存関係セクション参照）。
6. ~~`cells` のコンテナ型（`HashMap` vs `BTreeMap`）~~ → **解決**: `BTreeMap<CellRef, Cell>` を採用（Issue #87）。`iter_cells` の走査順が `json.rs` の `cells` 配列にそのまま反映されるため、`HashMap` のプロセスごとにランダムなハッシュシードに依存する走査順は、同一ファイルの2回のパース結果をテキスト差分で比較する用途で無関係なセル並び替えを大量の差分として見せてしまう問題があった。PoCによる実測（コードブロック直後の注記参照）で `BTreeMap` がCPU/ピークメモリの両面でも `HashMap` に劣らない（むしろ優る）ことを確認したうえで確定。`merged_regions` はJSON出力順に影響しないため対象外（`HashMap` のまま）。
7. **ハイパーリンク範囲と結合セルの相互作用、`finalize_hyperlinks`と`finalize_merges`の実行順序**: [resolve/hyperlink.md](../resolve/hyperlink.md) 自身のオープンクエスチョン1・2を参照。いずれも本ファイルの挙動に関わるが、実質的には`pipeline.rs`における`resolve::hyperlink::resolve`の呼び出し順序の話であり、`Sheet`のAPI自体の論点ではないためあちらで管理する。
