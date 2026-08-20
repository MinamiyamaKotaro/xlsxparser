# `resolve/hyperlink.rs` 設計書

*[English](hyperlink.en.md)*

`src/resolve/hyperlink.rs` に対応する設計書。[architecture.md](../architecture.md) が定義するフェーズ4の一部「セルのハイパーリンク範囲の遅延解決」を担う(Issue #95)。`pipeline.rs` が解決した(`r:id` → 生のTarget文字列。ZIP I/Oが必要なためpipeline.rs側で行う)`<hyperlinks>` の範囲リストを検証したうえで [`model::Sheet::finalize_hyperlinks`](../model/sheet.md) へ登録する。

[`resolve/merge.rs`](merge.md) とほぼ同じ構造を意図的に踏襲している。両者とも「永続的な鍵を持たない矩形範囲の集合を、対象セルへ1件ずつ展開することなく、既にセル化済みのデータと突き合わせて解決する」という同型の問題であり、本ファイルの「重複検証 → スイープライン解決」の2段構成は `resolve/merge.rs` + `Sheet::finalize_merges` で実証済みのアプローチをそのまま再利用したものである。

## 責務・スコープ

- `pipeline.rs` が構築したハイパーリンク範囲リスト(`Vec<model::sheet::HyperlinkRange>`。フェーズ3.5: `<hyperlink ref>` は `parse/worksheet.rs` がパースし、`r:id` はワークシート自身の `_rels` に対して `pipeline.rs` が解決する)を受け取り、範囲として妥当かを検証したうえで `Sheet::finalize_hyperlinks` をバッチ全体に対して1回呼び出す
- `Sheet::finalize_hyperlinks` のスイープライン解決が前提とする事前検証(開始・終了座標の大小関係の逆転、範囲同士の重複)を行う
- O(N²)の重複検証に入る前に `MAX_HYPERLINKS_PER_SHEET`(`resolve::merge::MAX_MERGE_REGIONS` と同種の増幅対策)を強制する
- **含まない責務**: `<hyperlink ref="...">` 属性からの `CellRef` ペアへの変換そのもの(`parse/worksheet.rs`)、`r:id` を `_rels` に対して解決し生のTarget文字列を得る処理そのもの(`pipeline.rs`。ZIP I/Oが必要なため——`architecture.md` の設計原則2により本ファイル自身のI/O非依存性を損なってはならない)、スイープライン解決アルゴリズムそのもの(`model::Sheet::finalize_hyperlinks`。[model/sheet.md](../model/sheet.md) 参照)

## 主要な型・関数(案)

```rust
use crate::error::Error;
use crate::model::sheet::{HyperlinkRange, Sheet};

/// 1シートあたりに受け入れる `<hyperlink>` エントリ数の上限。
/// `resolve::merge::MAX_MERGE_REGIONS` の値(20,000)と根拠を独自に導出せず
/// そのまま流用している。以下の `validate_range` は `resolve::merge` の
/// `validate_region` と全く同じO(N²)構造(新規範囲を検証済みの全範囲と
/// 比較する)であるため、同一のコスト曲線が当てはまる——
/// `resolve::merge` のオープンクエスチョン2の追記が記録した実測値
/// (N=40,000で約424ms、N=194,000で約10秒への外挿)がそのままこの上限にも
/// 適用でき、独自に再検証する必要がない。
pub(crate) const MAX_HYPERLINKS_PER_SHEET: usize = 20_000;

/// `ranges` を検証したうえで、バッチ全体を1回の `Sheet::finalize_hyperlinks`
/// 呼び出しで `sheet` へ登録する。`resolve::merge::resolve`(範囲ごとに
/// `Sheet::insert_merge` を呼び、最後に1回だけ `Sheet::finalize_merges` を
/// 呼ぶ)とは異なり、範囲単位の `Sheet` 呼び出しは存在しない——
/// `finalize_hyperlinks` は各範囲のプレースホルダセル挿入とスイープの
/// 両方を1回のパスで行うため、スイープ前の状態を他の呼び出し元に
/// 見せる理由が無い。
pub(crate) fn resolve(sheet: &mut Sheet, ranges: Vec<HyperlinkRange>) -> Result<(), Error> {
    if ranges.len() > MAX_HYPERLINKS_PER_SHEET {
        return Err(Error::TooManyHyperlinks {
            count: ranges.len(),
            limit: MAX_HYPERLINKS_PER_SHEET,
        });
    }
    let mut accepted: Vec<&HyperlinkRange> = Vec::with_capacity(ranges.len());
    for range in &ranges {
        validate_range(range, &accepted)?;
        accepted.push(range);
    }
    sheet.finalize_hyperlinks(ranges);
    Ok(())
}

/// 単一のハイパーリンク範囲について、開始・終了座標の大小関係と、既に
/// 検証を通過した範囲との重複を検証する。重複判定は `resolve::merge` の
/// `regions_overlap` と同じ、1ペアあたりO(1)の分離軸判定であり、セル単位
/// には展開しない——広大な範囲(`A1:XFD1048576`)でも1x1の範囲と同じ
/// コストで済む。
///
/// 検証するのはハイパーリンク範囲同士の重複のみ。ハイパーリンク範囲が
/// `MergedRegion` と重なることは正常かつ想定内である——結合とハイパー
/// リンクは同じ座標空間を占有し得る独立したOOXML概念であり、両者を
/// 相互排他にする理由は無い。
fn validate_range(range: &HyperlinkRange, accepted: &[&HyperlinkRange]) -> Result<(), Error> {
    if range.start.row > range.end.row || range.start.col > range.end.col {
        return Err(Error::InvalidHyperlinkRange {
            start: range.start.to_a1(),
            end: range.end.to_a1(),
            reason: "start must not be greater than end".to_string(),
        });
    }
    for other in accepted {
        if ranges_overlap(range, other) {
            return Err(Error::InvalidHyperlinkRange {
                start: range.start.to_a1(),
                end: range.end.to_a1(),
                // resolve::mergeの同種メッセージと異なり、競合した範囲自身の
                // 座標を含める——結合セルと違ってハイパーリンク範囲には
                // 見た目のセルレイアウトという追加の手がかりが読み手に
                // 無いため(Copilot PRレビュー、PR #96)。
                reason: format!(
                    "overlaps with another hyperlink range ({}:{})",
                    other.start.to_a1(),
                    other.end.to_a1()
                ),
            });
        }
    }
    Ok(())
}

fn ranges_overlap(a: &HyperlinkRange, b: &HyperlinkRange) -> bool {
    a.start.row <= b.end.row
        && a.end.row >= b.start.row
        && a.start.col <= b.end.col
        && a.end.col >= b.start.col
}
```

## 依存関係

- 依存先: [`model/sheet.rs`](../model/sheet.md)(`Sheet::finalize_hyperlinks`, `HyperlinkRange`)、[`error.rs`](../error.md)
- 依存元: `pipeline.rs`(`run` の1シートごとのループから直接呼ばれる。フェーズ3.5——`resolve::resolve_sheet` 経由ではない。`Vec<HyperlinkRange>` バッチの構築自体が `r:id` 解決のためのZIP I/Oを必要とし、本ファイルのI/O非依存な関数が呼ばれる前に完了している必要があるため)

## エラー処理方針

- 開始・終了座標が逆転した範囲、または他のハイパーリンク範囲と重複する範囲は、いずれも `Error::InvalidHyperlinkRange { start, end, reason }` として拒否する——`Error::InvalidMergedRange` と全く同じ形。
- 重複は「後勝ち」のようなタイブレークで解決せず、無条件に拒否する。`Sheet::finalize_hyperlinks` のスイープライン解決は、ある行で同時にアクティブな範囲同士の列範囲が互いに素であることを前提に、1回の二分探索で(一意な)対象範囲を見つける——これは `Sheet::finalize_merges` が既に依拠しているのと同じ前提である。この保証が無いと、列でソートされたアクティブ集合を使う近道が不健全になる——ある範囲の内側にありながら、より後から開始する別の範囲の内側でもあるクエリセルが、誤った範囲に(あるいはどの範囲にも)解決されかねず、「どちらが勝つか」が定義されない。実際のファイルで重複するハイパーリンク範囲が宣言されることは事実上あり得ない(Excel自身のUIには重複するハイパーリンクを作成する経路が無い。壊れたファイルが結合してしまいうる2つの独立したマージとは事情が異なる)ため、正当に発生しないはずのケースのためにタイブレークロジックを追加するより、`<mergeCell>` の重複に対して既に採った選択(無条件拒否)をそのまま踏襲する方を優先した。
- `MAX_HYPERLINKS_PER_SHEET` は、O(N²)の重複検証ループに入る前にチェックする——`resolve::merge::resolve` が既に採用している「高コストな処理の前にバッチサイズを拒否する」という順序をそのまま踏襲する。
- `panic` はしない(不正なハイパーリンク範囲は信頼できない外部入力=不正な `.xlsx` に起因しうるため)。
- 検証に失敗した時点で `resolve` 全体を中断し、範囲は一切登録しない(1件でも不正なら全体を拒否する。`resolve::merge::resolve` と同じfail closedの原則)。

## テスト方針

- 重複しない複数のハイパーリンク範囲が正しく登録されることの確認(`Sheet::hyperlink_at` が各範囲の起点セルだけでなく範囲内の全カバーセルを解決できることの結線テスト)
- 開始・終了座標が逆転した範囲に対し `Error::InvalidHyperlinkRange` を返すことの確認
- 2つのハイパーリンク範囲が一部でも重複する場合に `Error::InvalidHyperlinkRange` を返すことの確認
- 2つの範囲が座標軸上は近接するが実際には重ならない場合に、誤って重複と判定されないことの確認(`ranges_overlap` の境界値テスト。`resolve::merge` の同種テストと同型)
- ハイパーリンク範囲が無関係な `MergedRegion` と重なる場合に受理されることの確認(結合とハイパーリンクの重複は意図的にクロス検証しないことの確認)
- 極端に広大な単一ハイパーリンク範囲1件を検証しても、セル数に比例した時間がかからないことの確認
- 検証エラーが発生した場合、それより前に検証を通過した範囲も含めて一切登録されないことの確認
- 範囲リストが空の場合に何もせず `Ok(())` を返すことの確認
- 範囲数がちょうど `MAX_HYPERLINKS_PER_SHEET` なら受理され、1件超過すると`Error::TooManyHyperlinks` としてO(N²)ループに入る前に拒否されることの確認(`resolve::merge` の `region_count_over_the_limit_is_too_many_merged_ranges` と同型)
- エンドツーエンド(本モジュール単体ではなく `pipeline.rs` 自身のテストスイート経由): ハイパーリンク範囲を同時アクティブ行数が最大化するよう配置したシート + 多数の無関係セルが、セル数×範囲数に比例しないコストでJSON生成を完了することの確認(`tests/security.rs` の `sparse_merge_bounding_box_does_not_amplify_json_generation_cost` が結合セルについて守っているのと同種の回帰)

## 未決事項 / オープンクエスチョン

1. **ハイパーリンクの `ref` が結合セルの非起点(仮想)座標を指す場合の扱い**: 特別扱いしていない。`Sheet::finalize_hyperlinks` の起点プレースホルダ挿入は、実行時点で `cells` にその座標が存在するかだけを確認する(`resolve::merge::resolve` が `finalize_merges` で非起点の結合座標を既に取り除いた後に実行される)——ハイパーリンク範囲の起点がそのような座標に一致した場合、結合の起点セルへ畳み込まれるのではなく、新規の独立した空白セルとして再挿入されてしまい、`iter_cells`/JSON出力上は結合セルとは別の1セルとして現れる。実際のファイルでは発生しないと考えられる(Excelの UI には結合の仮想セルを起点と独立にアドレスする経路が無い)ため、今回は投機的に解決せず未対応のままとした。
2. **`resolve::hyperlink::resolve` の `resolve::resolve_sheet` に対する実行順序**: 現状は `pipeline.rs` から `resolve::resolve_sheet`(したがって結合の確定処理)の後に必ず呼ばれる——ハイパーリンク範囲の解決にはフェーズ3のストリーミングパースが `pending_hyperlinks` を出力し終えた後にしか開始できないZIP I/Oのステップ(`r:id` → Target)が必要なため。この順序こそがオープンクエスチョン1を成立させている当のものであり、ハイパーリンク解決を結合の確定より先に行う順序も検討したが、同じ種類の相互作用(既にハイパーリンクが付いたセルに結合の起点が一致するケース)が逆向きに発生するだけで解消にはならないため見送った。
3. **`MAX_HYPERLINKS_PER_SHEET` を `MAX_MERGE_REGIONS` から流用せず独自にチューニングすべきか**: 実際のシートでは結合よりもハイパーリンクの件数の方がはるかに少ないと想定される(ハイパーリンクは通常テンプレートによる一括生成ではなく手動編集で1セル/1範囲ずつ追加されるため)。したがって実務上は20,000は保守的な上限になる見込み。正当なファイルでこれを超える必要が生じた場合に再検討する。
