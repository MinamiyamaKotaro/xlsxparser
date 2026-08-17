# `resolve/shared_strings.rs` 設計書

*[English](shared_strings.en.md)*

`src/resolve/shared_strings.rs` に対応する設計書。[architecture.md](../architecture.md) が定義するフェーズ4の一部「共有文字列(SST)のインデックス解決」を担う。`t="s"` セルが保持する共有文字列テーブルへのインデックスを、実際の文字列（`model::CellValue::Text`）へ解決する。

## 責務・スコープ

- フェーズ3（`parse/worksheet.rs`）が記録した「共有文字列インデックス参照セルの保留リスト」を受け取り、`SharedStringTable`（`parse/shared_strings.rs` が構築、未設計）を引いて実文字列に解決し、対応する `Sheet` のセルへ書き戻す
- インデックスがテーブル範囲外の場合に `Error::SharedStringIndexOutOfBounds` を返す
- **含まない責務**: `sharedStrings.xml` のXMLパースおよび `SharedStringTable` の構築そのもの（`parse/shared_strings.rs`、未設計）、インラインストリング（`t="inlineStr"`）・数式文字列（`t="str"`）の解決（[model/cell.md](../model/cell.md) が述べる通りこれらも最終的に `CellValue::Text` へ統一されるが、`t="s"` と異なり参照テーブルを引く必要がないため、`parse/worksheet.rs` がストリーム中に直接 `CellValue::Text` として `Sheet` へ挿入できる。本ファイルは `t="s"` の遅延解決のみを扱う）

## 主要な型・関数（案）

```rust
use crate::error::Error;
use crate::model::cell::CellValue;
use crate::model::sheet::Sheet;
use crate::parse::shared_strings::SharedStringTable;
// PendingSharedStringはフェーズ3の出力データそのものであるため
// parse/worksheet.rsが定義する（PR #9レビューを反映。依存関係セクション参照）。
use crate::parse::worksheet::PendingSharedString;

/// `pending` の各エントリについて `table` から実文字列を引き、
/// `sheet` の対応セルへ `CellValue::Text` として書き戻す。
pub(crate) fn resolve(
    sheet: &mut Sheet,
    pending: &[PendingSharedString],
    table: &SharedStringTable,
) -> Result<(), Error> {
    for entry in pending {
        let text = table.get(entry.index).ok_or(Error::SharedStringIndexOutOfBounds {
            index: entry.index,
            len: table.len(),
        })?;
        // フェーズ3が同じ cell_ref で既にセルを挿入済みであることが前提
        // （resolve/mod.rs の呼び出し前提を参照）。
        let cell = sheet
            .get_mut(entry.cell_ref)
            .expect("pending shared string references a cell not inserted by parse/worksheet.rs");
        cell.value = Some(CellValue::Text(text.clone()));
    }
    Ok(())
}
```

## 依存関係

- 依存先: [`model/sheet.rs`](../model/sheet.md)（`Sheet::get_mut`）、[`model/cell.rs`](../model/cell.md)（`CellValue::Text`）、[`error.rs`](../error.md)、[`parse::shared_strings::SharedStringTable`](../parse/shared_strings.md)、[`parse::worksheet::PendingSharedString`](../parse/worksheet.md)
- 依存元: [`resolve/mod.rs`](mod.md)（`resolve_sheet` から呼び出される）

`get_mut` が `Option` ではなく `expect` でパニックしうる設計にしている点は、[model/sheet.md](../model/sheet.md) の「`get`/`get_mut` はセル不在を正常系として `Option` で表す」という方針と一見矛盾するように見えるが、ここでの前提は「セルが存在しない」ことがユーザ入力（XLSXファイル）由来ではなく、`parse/worksheet.rs` が `PendingSharedString` を記録した時点で対応する `insert_cell` を呼び忘れた場合にのみ発生する、クレート内部のプログラミングエラーである点が異なる（詳細はエラー処理方針参照）。

## エラー処理方針

- `table.get(index)` が `None` を返す場合（インデックスがテーブル範囲外）は `Error::SharedStringIndexOutOfBounds` を返す。これは信頼できない外部入力（不正な `.xlsx`）に起因しうるため、`panic` せず `Result` として伝播する。
- `sheet.get_mut(entry.cell_ref)` が `None` を返すケース（`PendingSharedString` が指すセルが `Sheet` に存在しない）は、外部入力の不正ではなく `parse/worksheet.rs` の実装不備（`t="s"` セル検出時に `PendingSharedString` の記録と `insert_cell` の呼び出しを対にする不変条件が破られた場合）を意味するため、`Result` ではなく `expect` によるパニックとする。この不変条件は `parse/worksheet.rs` の設計時に明文化する（オープンクエスチョン2参照）。

## テスト方針

- 正当なインデックスを持つ `PendingSharedString` が、`SharedStringTable` の対応する文字列で正しく `CellValue::Text` に解決されることの確認
- テーブル範囲外のインデックス（`table.len()` と同値、または大きく超える値）を持つ場合に `Error::SharedStringIndexOutOfBounds` を返し、`index`/`len` の値が正しいことの確認
- 同一の文字列を参照する複数の `PendingSharedString` が解決された際、対応する `CellValue::Text` の `Arc<str>` が `Arc::ptr_eq` で同一（アロケーション重複がない）ことの確認（[model/cell.md](../model/cell.md) の `Arc<str>` 設計方針との結線確認）
- `pending` が空リストの場合に何もせず `Ok(())` を返すことの確認

## 未決事項 / オープンクエスチョン

1. ~~`SharedStringTable` の型・配置場所~~ → **解決**: [`parse/shared_strings.rs`](../parse/shared_strings.md) が `Vec<Arc<str>>` をラップした型として定義し、`get(index) -> Option<&Arc<str>>` / `len()` を提供する。
2. ~~`parse/worksheet.rs` との不変条件の明文化~~ → **解決**: 「`t="s"` セルを検出したら `PendingSharedString` の記録と空の `Cell`（`value: None`）の `insert_cell` を必ず対で行う」という契約は [`parse/worksheet.rs`](../parse/worksheet.md) 側のドキュメントを正として記載する。
3. ~~数式セル（`t="str"`）とインラインストリング（`t="inlineStr"`）の解決タイミングの妥当性~~ → **確認済み**: [`parse/worksheet.rs`](../parse/worksheet.md) の設計で、ストリーム中に直接 `CellValue::Text` へ解決する前提のまま設計を確定した。`Arc<str>` へのラップコストの実測は実装時のプロファイリングに委ねる。
