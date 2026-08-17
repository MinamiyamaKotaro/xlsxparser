# `resolve/style.rs` 設計書

*[English](style.en.md)*

`src/resolve/style.rs` に対応する設計書。[architecture.md](../architecture.md) が定義するフェーズ4の一部「セルスタイルの適用」を担う。[`model/style.rs`](../model/style.md) が定義する `StyleSheet` から得たスタイル定義を各セルへ適用し、あわせて数値書式（numFmt）が日付/時刻を表す場合の `CellValue::Number` → `CellValue::DateTime` 変換を行う。

## 責務・スコープ

- フェーズ3（`parse/worksheet.rs`）が記録した「スタイルID参照セルの保留リスト」を受け取り、[`model::style::StyleSheet`](../model/style.md) を引いて解決済みスタイル（`ResolvedStyle`）を各セルの `style: Option<Arc<ResolvedStyle>>` へ設定する
- 適用したスタイルの数値書式（numFmt）が日付/時刻書式であると判定した場合、対象セルの `CellValue::Number` を `CellValue::DateTime` へ変換する。変換できない値（負値・`NaN`・`Infinity`・Excelの表現範囲外など）の場合は変換をスキップし、`CellValue::Number` を維持したまま処理を継続する（フォールバック。PR #8 レビュー指摘を反映してオープンクエスチョン3を解決。詳細はエラー処理方針参照）
- スタイルIDがスタイル定義の範囲外の場合に `Error::InvalidStyleId` を返す
- **含まない責務**: `styles.xml` のXMLパースおよび `ResolvedStyle` を構築するロジックそのもの（`parse/styles.rs`、未設計。本ファイルは構築済みの `StyleSheet` を受け取る前提とする）、`ResolvedStyle` / `StyleSheet` / `StyleId` の型定義そのもの（[`model/style.rs`](../model/style.md) へ移動。PR #8 レビュー指摘を反映してオープンクエスチョン1を解決）、日付/時刻書式かどうかの具体的な numFmt コード判定ルール自体の実装（本ファイルは判定結果を `ResolvedStyle` が既に保持している前提とする。オープンクエスチョン2参照）

## 主要な型・関数（案）

```rust
use crate::error::Error;
use crate::model::cell::{CellValue, DateTimeValue};
use crate::model::sheet::{CellRef, Sheet};
use crate::model::style::{ResolvedStyle, StyleId, StyleSheet};

/// フェーズ3が `s`（style index）属性を持つセルを検出した時点で記録する保留エントリ。
#[derive(Debug, Clone, Copy)]
pub struct PendingStyle {
    pub cell_ref: CellRef,
    pub style_id: StyleId,
}

/// `pending` の各エントリについて `stylesheet` から `ResolvedStyle` を引き、
/// `sheet` の対応セルへ設定する。あわせて `is_date_time` な書式が
/// `CellValue::Number` に適用された場合、`CellValue::DateTime` へ変換する。
pub(crate) fn resolve(
    sheet: &mut Sheet,
    pending: &[PendingStyle],
    stylesheet: &StyleSheet,
) -> Result<(), Error> {
    for entry in pending {
        let resolved = stylesheet
            .get(&entry.style_id)
            .ok_or(Error::InvalidStyleId(entry.style_id))?
            .clone();
        let cell = sheet
            .get_mut(entry.cell_ref)
            .expect("pending style references a cell not inserted by parse/worksheet.rs");
        if resolved.is_date_time {
            if let Some(CellValue::Number(serial)) = cell.value {
                // 変換できない場合は CellValue::Number のまま維持する
                // （フォールバック。エラー処理方針参照）。
                if let Some(dt) = serial_to_date_time(serial) {
                    cell.value = Some(CellValue::DateTime(dt));
                }
            }
        }
        cell.style = Some(resolved);
    }
    Ok(())
}

/// Excelのシリアル値（1900年うるう年バグを含む日付エポック）を
/// `DateTimeValue` へ変換する。負値・`NaN`・`Infinity`・Excelの表現範囲
/// （最大 9999年12月31日相当）を超える値など、変換不能な値に対しては
/// `None` を返す（エラーにはしない。エラー処理方針参照）。具体的な変換式
/// は [model/cell.md オープンクエスチョン4](../model/cell.md) と連動して未確定。
fn serial_to_date_time(serial: f64) -> Option<DateTimeValue> {
    let _ = serial;
    unimplemented!()
}
```

## 依存関係

- 依存先: [`model/sheet.rs`](../model/sheet.md)（`Sheet::get_mut`, `CellRef`）、[`model/cell.rs`](../model/cell.md)（`CellValue`, `DateTimeValue`）、[`model/style.rs`](../model/style.md)（`ResolvedStyle`, `StyleSheet`, `StyleId`。PR #8 レビュー指摘を反映し本ファイルから移動）、[`error.rs`](../error.md)
- 依存元: [`resolve/mod.rs`](mod.md)（`resolve_sheet` から呼び出される）

`StyleSheet` / `ResolvedStyle` / `StyleId` を `resolve/style.rs` 自身ではなく [`model/style.rs`](../model/style.md) に定義したことで、`parse/styles.rs`（未設計。`StyleSheet` を構築する主体）と `resolve/style.rs`（適用する主体）がいずれも `model/` にのみ依存し、互いを直接知らない構造になる（PR #8 レビュー指摘を反映。詳細は[model/style.md](../model/style.md)参照）。

[`resolve/shared_strings.rs`](shared_strings.md) と同様、`sheet.get_mut` が `None` を返すケースは `parse/worksheet.rs` 側の不変条件違反（クレート内部のプログラミングエラー）とみなし `expect` を用いる（詳細はエラー処理方針参照）。

`CellValue::Number` から `CellValue::DateTime` への変換をスタイル適用と同一の関数内で行っている理由: numFmt（＝スタイル情報）を見なければ、あるセルの `f64` がプレーンな数値なのか日付シリアル値なのかを判別できないため、[model/cell.md](../model/cell.md) が既に述べる通りこの変換は本ファイル（`resolve/style.rs`）の責務である。[`resolve/shared_strings.rs`](shared_strings.md) の解決が本処理より先に走る前提（[resolve/mod.md](mod.md) の呼び出し順序）だが、`CellValue::Text` に対しては `is_date_time` を見ても変換を行わない（`if let Some(CellValue::Number(..))` によるパターンマッチで自然に除外される）ため、呼び出し順序が入れ替わっても本処理自体の正しさには影響しない。

## エラー処理方針

- `stylesheet.get(style_id)` が `None` を返す場合（`cellXfs` の範囲外インデックス）は `Error::InvalidStyleId` を返す。信頼できない外部入力に起因しうるため `panic` しない。
- `sheet.get_mut(entry.cell_ref)` が `None` を返すケースは [shared_strings.md エラー処理方針](shared_strings.md) と同じ理由（`parse/worksheet.rs` の不変条件違反）で `expect` によるパニックとする。
- **日付変換の失敗はエラーとして扱わずフォールバックする**: `serial_to_date_time` が `None` を返す場合（負値・`NaN`・`Infinity`・Excelの表現範囲外など、日付として解釈できない値）、`Error` を生成して呼び出し元へ伝播させることはしない。対象セルの `CellValue` を `Number(serial)` のまま維持し、解決処理全体は正常系として継続する。1セルの日付解釈失敗のためにドキュメント全体のパースが失敗するのは過剰に脆弱であり、当該セルの値自体（数値としては正しい）は失われないため、消費側でnumFmtを見て独自に再解釈する余地も残せる（PR #8 レビュー指摘を反映してオープンクエスチョン3を解決）。この方針は [merge.md](merge.md) や [container/sanitize.md](../container/sanitize.md) が採用する「不正入力は fail closed で全体拒否する」という方針とは異なる点に注意。両者の違いは、後者が「セキュリティ上の脅威（Zip Bomb/Slip）や構造的な不整合（結合範囲の重複）」を扱うのに対し、本ケースは「個々のセル値の解釈の緩やかな失敗」であり、ドキュメント全体の整合性を損なわない点にある。

## テスト方針

- 正当なスタイルIDを持つ `PendingStyle` が、`StyleSheet` の対応する `ResolvedStyle` で `Cell.style` に正しく設定されることの確認
- 存在しないスタイルID（`StyleSheet` の範囲外）を持つ場合に `Error::InvalidStyleId` を返すことの確認
- `is_date_time: true` なスタイルが `CellValue::Number` を持つセルに適用され、変換可能な値の場合に `CellValue::DateTime` へ変換されることの確認
- **`is_date_time: true` なスタイルが `CellValue::Number` を持つセルに適用されたが、値が変換不能（負値・`NaN`・`Infinity`など）な場合に、`Err` を返さず `CellValue::Number` のまま維持されることの確認**（PR #8 レビューで追加したフォールバック仕様の回帰テスト観点）
- `is_date_time: true` なスタイルが `CellValue::Text` / `CellValue::Boolean` など数値以外を持つセルに適用された場合、`value` が変換されずそのまま保持されることの確認（誤変換防止の回帰テスト観点）
- `is_date_time: false` な通常のスタイルが適用された場合、`value` が変換されず `style` のみ設定されることの確認
- 同一の `ResolvedStyle` を複数セルへ適用した際、`Cell.style` の `Arc` が `Arc::ptr_eq` で同一（アロケーション重複がない）ことの確認（[model/cell.md](../model/cell.md) の `Arc` 設計方針との結線確認）
- `pending` が空リストの場合に何もせず `Ok(())` を返すことの確認

## 未決事項 / オープンクエスチョン

1. ~~`ResolvedStyle` / `StyleSheet` / `StyleId` の最終的な配置場所~~ → **解決**: [`model/style.rs`](../model/style.md) を新設し、そちらに定義する。`parse/styles.rs`（構築主体）と `resolve/style.rs`（適用主体）の双方が `model/` にのみ依存する構造とすることで、レイヤー間の独立性を保つ（PR #8 レビュー指摘を反映）。
2. ~~日付/時刻書式の判定ロジックの置き場所~~ → **解決**: [`parse/styles.rs`](../parse/styles.md) 側で判定する（OOXMLの numFmt判定を含む）。本ファイルは引き続き `ResolvedStyle.is_date_time` を既に判定済みの値として受け取るのみで、判定ロジックそのものは持たない。判定ヒューリスティックの精度自体は [parse/styles.md オープンクエスチョン2](../parse/styles.md) として引き続き未解決。
3. ~~`serial_to_date_time` の実装~~ → **一部解決**: 変換不能な値に対しては `Error` を返さず `None` を返し、呼び出し側（本ファイルの `resolve`）が `CellValue::Number` を維持するフォールバックとする方針を確定した（PR #8 レビュー指摘を反映）。ただし変換式そのもの（1900年うるう年バグの扱いを含む）は [model/cell.md オープンクエスチョン4](../model/cell.md) の `DateTimeValue` 型確定と合わせて未確定のまま。
4. **フォント/塗りつぶし/罫線などの具体的なスタイル要素**: [model/style.md オープンクエスチョン1](../model/style.md) と同一の論点（未解決）。要求仕様書がセルスタイルとしてどこまでの要素をJSON出力に含める必要があるかは `json.rs` の設計、または要求仕様書自体の詳細化と合わせて確定させる。
