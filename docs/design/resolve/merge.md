# `resolve/merge.rs` 設計書

*[English](merge.en.md)*

`src/resolve/merge.rs` に対応する設計書。[architecture.md](../architecture.md) が定義するフェーズ4の一部「結合セルの遅延解決・エイリアス参照マッピング」を担う。要求仕様書3.2（結合セルの透過的アクセス）を実現するため、`<mergeCells>` の結合範囲リストを検証したうえで [`model::Sheet::insert_merge`](../model/sheet.md) へ登録する。

## 責務・スコープ

- フェーズ3（`parse/worksheet.rs`）が収集した `<mergeCells>` の結合範囲リスト（`Vec<model::sheet::MergedRegion>`）を受け取り、範囲として妥当かを検証したうえで `Sheet::insert_merge` を順に呼び出す
- 範囲同士の重複、開始・終了座標の大小関係の逆転など、`Sheet::insert_merge` 自身が前提とする「渡された範囲は妥当である」という契約（[model/sheet.md エラー処理方針](../model/sheet.md)）を満たすための事前検証を行う
- **含まない責務**: `<mergeCells ref="A1:C3">` 属性からの `MergedRegion`（`CellRef::from_a1` を用いた `start`/`end` への変換）の構築そのもの（`parse/worksheet.rs`、未設計。本ファイルは既に `MergedRegion` へ変換済みのリストを受け取る前提とする）、結合起点セルへのエイリアス解決ロジックそのもの（`model::Sheet::get` / `insert_merge` 内部の実装。[model/sheet.md](../model/sheet.md) 参照）

## 主要な型・関数（案）

```rust
use std::collections::HashSet;

use crate::error::Error;
use crate::model::sheet::{CellRef, MergedRegion, Sheet};

/// `regions` を検証しつつ `sheet` へ順に登録する。
/// 呼び出し順（リストの先頭から）が登録順となり、同一セルを含む範囲が
/// 複数存在した場合は [model/sheet.md](../model/sheet.md) オープンクエスチョン3
/// の「後勝ち上書き」がそのまま適用される。ただし本関数は明確な範囲の重複
/// （同一セルに対する2つ以上の異なる起点セル指定）を検証エラーとして拒否
/// するため、実際に重複登録が `Sheet` 側まで到達することはない
/// （オープンクエスチョン1参照）。
pub(crate) fn resolve(sheet: &mut Sheet, regions: Vec<MergedRegion>) -> Result<(), Error> {
    let mut occupied: HashSet<CellRef> = HashSet::new();
    for region in &regions {
        validate_region(region, &occupied)?;
        mark_occupied(region, &mut occupied);
    }
    for region in regions {
        sheet.insert_merge(region);
    }
    Ok(())
}

/// 単一の結合範囲が構造的に妥当か（開始・終了座標の大小関係、既存の
/// 結合範囲との重複）を検証する。
fn validate_region(region: &MergedRegion, occupied: &HashSet<CellRef>) -> Result<(), Error> {
    if region.start.row > region.end.row || region.start.col > region.end.col {
        return Err(Error::InvalidMergedRange {
            start: region.start.to_a1(),
            end: region.end.to_a1(),
            reason: "start must not be greater than end".to_string(),
        });
    }
    for row in region.start.row..=region.end.row {
        for col in region.start.col..=region.end.col {
            if occupied.contains(&CellRef { row, col }) {
                return Err(Error::InvalidMergedRange {
                    start: region.start.to_a1(),
                    end: region.end.to_a1(),
                    reason: "overlaps with another merged range".to_string(),
                });
            }
        }
    }
    Ok(())
}

fn mark_occupied(region: &MergedRegion, occupied: &mut HashSet<CellRef>) {
    for row in region.start.row..=region.end.row {
        for col in region.start.col..=region.end.col {
            occupied.insert(CellRef { row, col });
        }
    }
}
```

## 依存関係

- 依存先: [`model/sheet.rs`](../model/sheet.md)（`Sheet::insert_merge`, `MergedRegion`, `CellRef`）、[`error.rs`](../error.md)
- 依存元: [`resolve/mod.rs`](mod.md)（`resolve_sheet` から呼び出される）

`validate_region` が座標を都度 `HashSet<CellRef>` へ展開して重複判定する実装は、大きな結合範囲（例えば方眼紙Excelでの数百セル規模の結合）が多数存在する場合に計算量・メモリ効率上の懸念がある（オープンクエスチョン2参照）。

## エラー処理方針

- 開始・終了座標の大小関係が逆転している範囲、既存の範囲と重複する範囲は、いずれも `Error::InvalidMergedRange { start, end, reason }` として拒否する（[model/sheet.md エラー処理方針](../model/sheet.md) が述べる「`insert_merge` 呼び出し前に `resolve/merge.rs` 側で検証する」方針をそのまま実装する）。
- `panic` はしない（結合範囲の不正は信頼できない外部入力＝不正な `.xlsx` に起因しうるため）。
- 検証に失敗した時点で `resolve` 全体を中断し、それ以降の範囲は登録しない（1件でも不正なら全体を拒否する。[container/sanitize.md](../container/sanitize.md) の `validate_entry_path` と同じ fail closed の原則）。

## テスト方針

- 重複しない複数の結合範囲が正しく `Sheet::insert_merge` へ登録されることの確認（`Sheet::get` で仮想セル座標から起点セルが引けることの結線テスト）
- 開始・終了座標が逆転した範囲（例: `start: C3, end: A1`）に対し `Error::InvalidMergedRange` を返すことの確認
- 2つの結合範囲が一部でも重複する場合（例: `A1:C3` と `B2:D4`）に `Error::InvalidMergedRange` を返すことの確認
- 検証エラーが発生した場合、それより前に検証を通過した範囲も含めて `Sheet` へ一切登録されないことの確認（全体拒否の確認）
- 結合範囲リストが空の場合に何もせず `Ok(())` を返すことの確認
- 1x1の結合範囲（実質的に結合ではない自明なケース）が正しく処理されることの確認（境界値）

## 未決事項 / オープンクエスチョン

1. **重複検証を `Sheet::insert_merge` 側ではなく本ファイル側に置く設計の妥当性**: [model/sheet.md](../model/sheet.md) は「`insert_merge` を複数回呼んだ場合は単純に上書きする実装を想定」と述べており、本ファイルの検証層がなければ重複範囲はサイレントに後勝ち上書きされる。検証を挟むことでこの挙動を「エラー」に変える設計判断だが、意図的に重複を許容したい将来のユースケース（例えば壊れた `.xlsx` を可能な限り読み進めたいエラー耐性モード）が要求仕様に含まれるかは未確定。
2. **重複判定の計算量**: 現在の `HashSet<CellRef>` への座標展開はO(結合範囲内のセル数)のメモリ・時間を要する。結合範囲が広大（例: A1:XFD1048576 のような極端なケース）な場合に問題化しうるため、区間木（interval tree）等より効率的なデータ構造への置き換えが必要かは、実データでの検証後に判断する。
3. **`MergedRegion` を `Vec` として一括受け渡しする設計の妥当性**: [resolve/mod.md オープンクエスチョン3](mod.md) と同様、`parse/worksheet.rs` が未設計のため、`<mergeCells>` 要素（`worksheet.xml` 内で通常は全行データの後、末尾近くに出現する）をストリームのどの時点で `Vec<MergedRegion>` として確定できるかは `parse/worksheet.rs` の設計時に確定させる。
