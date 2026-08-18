# `resolve/merge.rs` 設計書

*[English](merge.en.md)*

`src/resolve/merge.rs` に対応する設計書。[architecture.md](../architecture.md) が定義するフェーズ4の一部「結合セルの遅延解決・エイリアス参照マッピング」を担う。要求仕様書3.2（結合セルの透過的アクセス）を実現するため、`<mergeCells>` の結合範囲リストを検証したうえで [`model::Sheet::insert_merge`](../model/sheet.md) へ登録する。

## 責務・スコープ

- フェーズ3（`parse/worksheet.rs`）が収集した `<mergeCells>` の結合範囲リスト（`Vec<model::sheet::MergedRegion>`）を受け取り、範囲として妥当かを検証したうえで `Sheet::insert_merge` を順に呼び出す
- 範囲同士の重複、開始・終了座標の大小関係の逆転など、`Sheet::insert_merge` 自身が前提とする「渡された範囲は妥当である」という契約（[model/sheet.md エラー処理方針](../model/sheet.md)）を満たすための事前検証を行う
- **含まない責務**: `<mergeCells ref="A1:C3">` 属性からの `MergedRegion`（`CellRef::from_a1` を用いた `start`/`end` への変換）の構築そのもの（`parse/worksheet.rs`、未設計。本ファイルは既に `MergedRegion` へ変換済みのリストを受け取る前提とする）、結合起点セルへのエイリアス解決ロジックそのもの（`model::Sheet::get` / `insert_merge` 内部の実装。[model/sheet.md](../model/sheet.md) 参照）

## 主要な型・関数（案）

```rust
use crate::error::Error;
use crate::model::sheet::{MergedRegion, Sheet};

/// `regions` を検証しつつ `sheet` へ順に登録する。
/// 呼び出し順（リストの先頭から）が登録順となり、同一セルを含む範囲が
/// 複数存在した場合は [model/sheet.md](../model/sheet.md) オープンクエスチョン3
/// の「後勝ち上書き」がそのまま適用される。ただし本関数は明確な範囲の重複
/// （同一セルに対する2つ以上の異なる起点セル指定）を検証エラーとして拒否
/// するため、実際に重複登録が `Sheet` 側まで到達することはない
/// （オープンクエスチョン1参照）。
///
/// 全範囲を登録し終えた後、[`Sheet::finalize_merges`](../model/sheet.md)
/// を呼び出し、全セルを起点へ一括解決する——これにより、結合セルが
/// どう配置されていても `json.rs` の後続の `iter_cells` 呼び出しが
/// 高速なままになる（Issue #43。詳細は `model/sheet.md` の
/// 「修正: `finalize_merges`」を参照）。
pub(crate) fn resolve(sheet: &mut Sheet, regions: Vec<MergedRegion>) -> Result<(), Error> {
    let mut accepted: Vec<MergedRegion> = Vec::with_capacity(regions.len());
    for region in &regions {
        validate_region(region, &accepted)?;
        accepted.push(*region);
    }
    for region in regions {
        sheet.insert_merge(region);
    }
    sheet.finalize_merges();
    Ok(())
}

/// 単一の結合範囲が構造的に妥当か（開始・終了座標の大小関係、既に検証を
/// 通過した結合範囲との重複）を検証する。
///
/// 重複判定はセル単位に展開せず、矩形同士の幾何的な交差判定（O(1)）を
/// `accepted` の各要素に対して行う（1件あたりO(検証済み件数)）ことで、
/// 結合範囲が広大な場合（例: `A1:XFD1048576`）でもセル数（10億超）に
/// 比例した計算量が発生しないようにする（PR #8 レビュー指摘を反映して
/// オープンクエスチョン2を解決）。
fn validate_region(region: &MergedRegion, accepted: &[MergedRegion]) -> Result<(), Error> {
    if region.start.row > region.end.row || region.start.col > region.end.col {
        return Err(Error::InvalidMergedRange {
            start: region.start.to_a1(),
            end: region.end.to_a1(),
            reason: "start must not be greater than end".to_string(),
        });
    }
    for other in accepted {
        if regions_overlap(region, other) {
            return Err(Error::InvalidMergedRange {
                start: region.start.to_a1(),
                end: region.end.to_a1(),
                reason: "overlaps with another merged range".to_string(),
            });
        }
    }
    Ok(())
}

/// 2つの矩形範囲（結合範囲）が座標軸上で重なりを持つかをO(1)で判定する
/// （分離軸判定: いずれかの軸で完全に分離していれば重ならない）。
fn regions_overlap(a: &MergedRegion, b: &MergedRegion) -> bool {
    a.start.row <= b.end.row
        && a.end.row >= b.start.row
        && a.start.col <= b.end.col
        && a.end.col >= b.start.col
}
```

## 依存関係

- 依存先: [`model/sheet.rs`](../model/sheet.md)（`Sheet::insert_merge`, `Sheet::finalize_merges`, `MergedRegion`）、[`error.rs`](../error.md)
- 依存元: [`resolve/mod.rs`](mod.md)（`resolve_sheet` から呼び出される）

`validate_region` は結合範囲を `HashSet<CellRef>` へ展開せず、既に検証を通過した範囲（`accepted: &[MergedRegion]`）との矩形交差判定のみで重複を検出する。1件あたりの判定コストは範囲の面積（セル数）に依存せずO(1)、`N`件の範囲全体を検証する総コストはO(N²)（各範囲がそれまでに検証済みの範囲と比較するため）に抑えられる（PR #8 レビュー指摘を反映。旧設計の `HashSet<CellRef>` 展開では `A1:XFD1048576` のような広大な範囲1件だけで10億セル超のループが発生しCPUをハングアップさせうる問題があった）。

## エラー処理方針

- 開始・終了座標の大小関係が逆転している範囲、既存の範囲と重複する範囲は、いずれも `Error::InvalidMergedRange { start, end, reason }` として拒否する（[model/sheet.md エラー処理方針](../model/sheet.md) が述べる「`insert_merge` 呼び出し前に `resolve/merge.rs` 側で検証する」方針をそのまま実装する）。
- `panic` はしない（結合範囲の不正は信頼できない外部入力＝不正な `.xlsx` に起因しうるため）。
- 検証に失敗した時点で `resolve` 全体を中断し、それ以降の範囲は登録しない（1件でも不正なら全体を拒否する。[container/sanitize.md](../container/sanitize.md) の `validate_entry_path` と同じ fail closed の原則）。

## テスト方針

- 重複しない複数の結合範囲が正しく `Sheet::insert_merge` へ登録されることの確認（`Sheet::get` で仮想セル座標から起点セルが引けることの結線テスト）
- 開始・終了座標が逆転した範囲（例: `start: C3, end: A1`）に対し `Error::InvalidMergedRange` を返すことの確認
- 2つの結合範囲が一部でも重複する場合（例: `A1:C3` と `B2:D4`）に `Error::InvalidMergedRange` を返すことの確認
- 2つの結合範囲が座標軸上は近接するが実際には重ならない場合（例: `A1:B2` と `C1:D2`。列が隣接するのみ）に、誤って重複と判定されないことの確認（`regions_overlap` の境界値テスト）
- **極端に広大な単一結合範囲（例: `A1:XFD1048576`）1件を検証しても、セル数に比例した時間がかからず即座に完了することの確認**（PR #8 レビューで指摘されたDoS耐性の回帰テスト観点）
- 検証エラーが発生した場合、それより前に検証を通過した範囲も含めて `Sheet` へ一切登録されないことの確認（全体拒否の確認）
- 結合範囲リストが空の場合に何もせず `Ok(())` を返すことの確認
- 1x1の結合範囲（実質的に結合ではない自明なケース）が正しく処理されることの確認（境界値）
- `resolve` が全範囲登録後に `Sheet::finalize_merges` を呼ぶことの確認(`model/sheet.md` の `finalize_merges` テスト群と、`tests/fixtures/security.rs` の `sparse_merge_bounding_box_amplification` によるエンドツーエンドのフィクスチャで間接的にカバーする。本モジュール単体ではなくパイプライン全体を通して検証する)

## 未決事項 / オープンクエスチョン

1. **重複検証を `Sheet::insert_merge` 側ではなく本ファイル側に置く設計の妥当性**: [model/sheet.md](../model/sheet.md) は「`insert_merge` を複数回呼んだ場合は単純に上書きする実装を想定」と述べており、本ファイルの検証層がなければ重複範囲はサイレントに後勝ち上書きされる。検証を挟むことでこの挙動を「エラー」に変える設計判断だが、意図的に重複を許容したい将来のユースケース（例えば壊れた `.xlsx` を可能な限り読み進めたいエラー耐性モード）が要求仕様に含まれるかは未確定。
2. ~~重複判定の計算量~~ → **解決**: セル単位の `HashSet<CellRef>` 展開ではなく、矩形同士の幾何的交差判定（分離軸判定）をO(1)で行う設計に変更した。検証済みのN件の範囲に対して新規の1件を検証するコストはO(N)、全体でO(N²)に収まり、結合範囲の面積（セル数）に依存しない。範囲の件数Nが非常に多い場合（例: 数万件規模）はソート+スイープライン法でO(N log N)へさらに改善する余地があるが、実務上のExcelファイルで結合範囲が万単位に達するケースは稀と想定されるため、現時点ではO(N²)のシンプルな実装で十分とする（PR #8 レビュー指摘を反映）。
   **追記（2026-08-17、[セキュリティコードレビュー Finding 1](../../security/code-review.md) を受けて）**: 「実務上のファイルでは稀」という上記の前提は、攻撃者が意図的に大量の `<mergeCell>` を詰め込んだ非正規のファイルを作る場合には成立しない。`<mergeCell>` 1件は約20〜30バイトしかないため、Zip Bomb対策のバイト数上限（既定512MiB/エントリ）は範囲件数Nを実質的に有界にできず、O(N²)のまま数百KB〜数MBのファイルで数十秒〜数分のCPU拘束を引き起こせることを実測で確認した。根本対策（スイープライン法へのO(N log N)化）は見送ったまま、`resolve::merge::MAX_MERGE_REGIONS`（既定20,000件）による防御的な件数上限を追加し、上限超過時はO(N²)ループに入る前に `Error::TooManyMergedRanges` を返すようにした。
   **第2の追記（2026-08-18、Issue #43）**: `MAX_MERGE_REGIONS` は**本ファイル自身**のO(N²)検証コストを抑えるものだが、これとは別に、それまで気づかれていなかったコストが呼び出しスタックの一段上——`model::Sheet` の `get`/`get_mut`/`iter_cells` によるセル単位のエイリアス解決——に存在していた。これは `merge_bounds` によるO(1)事前チェック（[model/sheet.md](../model/sheet.md)）でも部分的にしか防げない。既存の全ての上限内に収まるファイルで、`json.rs` のセル走査だけで最大数十秒のCPU時間を要することを実測した。本ファイル自身のO(N²)(上記の追記でスイープライン法への書き換えを検討し見送った箇所)とは異なり、今回の対策は**まさにそのスイープライン法への書き換え**である——ただし `validate_region` ではなく、全範囲登録後に `resolve` が新たに呼び出す `Sheet::finalize_merges` に適用した。詳細（3種の単純な対策が実測により効果が無いと判明した経緯を含む）は [model/sheet.md](../model/sheet.md) の「修正: `finalize_merges`」を参照。
3. **`MergedRegion` を `Vec` として一括受け渡しする設計の妥当性**: [resolve/mod.md オープンクエスチョン3](mod.md) と同様、`parse/worksheet.rs` が未設計のため、`<mergeCells>` 要素（`worksheet.xml` 内で通常は全行データの後、末尾近くに出現する）をストリームのどの時点で `Vec<MergedRegion>` として確定できるかは `parse/worksheet.rs` の設計時に確定させる。
