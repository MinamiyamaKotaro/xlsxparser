# `resolve/style.rs` 設計書

*[English](style.en.md)*

`src/resolve/style.rs` に対応する設計書。[architecture.md](../architecture.md) が定義するフェーズ4の一部「セルスタイルの適用」を担う。`styles.xml`（`parse/styles.rs` が構築、未設計）から得たスタイル定義を各セルへ適用し、あわせて数値書式（numFmt）が日付/時刻を表す場合の `CellValue::Number` → `CellValue::DateTime` 変換を行う。[model/cell.md](../model/cell.md) オープンクエスチョン3（`ResolvedStyle` の定義場所）を解決する。

## 責務・スコープ

- フェーズ3（`parse/worksheet.rs`）が記録した「スタイルID参照セルの保留リスト」を受け取り、`StyleSheet` を引いて解決済みスタイル（`ResolvedStyle`）を各セルの `style: Option<Arc<ResolvedStyle>>` へ設定する
- 適用したスタイルの数値書式（numFmt）が日付/時刻書式であると判定した場合、対象セルの `CellValue::Number` を `CellValue::DateTime` へ変換する（[model/cell.md](../model/cell.md) が述べる `DateTime` バリアントの生成元）
- スタイルIDがスタイル定義の範囲外の場合に `Error::InvalidStyleId` を返す
- **含まない責務**: `styles.xml` のXMLパースおよび `fonts`/`fills`/`borders`/`numFmts`/`cellXfs` から `ResolvedStyle` を構築するロジックそのもの（`parse/styles.rs`、未設計。本ファイルは構築済みの `StyleSheet` を受け取る前提とする）、日付/時刻書式かどうかの具体的な numFmt コード判定ルール自体の実装（本ファイルは判定結果を `ResolvedStyle` が既に保持している前提とする。オープンクエスチョン2参照）

## 主要な型・関数（案）

```rust
use std::collections::HashMap;
use std::sync::Arc;

use crate::error::Error;
use crate::model::cell::{CellValue, DateTimeValue};
use crate::model::sheet::{CellRef, Sheet};

/// `cellXfs` のインデックス（スタイルID）。[error.rs](../error.md) の
/// `Error::InvalidStyleId(u32)` と型を揃える。
pub type StyleId = u32;

/// スタイルID解決後の書式情報。[model/cell.md オープンクエスチョン3](../model/cell.md)
/// を解決し、本ファイル側に定義する（`parse/styles.rs` 設計時に構成要素
/// を見直す可能性あり。オープンクエスチョン1参照）。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedStyle {
    /// この書式が日付/時刻を表すか。`parse/styles.rs` が `numFmts` の
    /// コード文字列（組み込み・カスタム双方）を解釈し、あらかじめ
    /// 判定した結果をここに格納しておく想定（オープンクエスチョン2参照）。
    pub is_date_time: bool,
    // font/fill/border 等の具体的なフィールドは parse/styles.rs の設計時に確定させる。
}

/// `cellXfs` インデックスから `ResolvedStyle` を引くテーブル。
/// `parse/styles.rs` が構築する想定（オープンクエスチョン1参照）。
pub type StyleSheet = HashMap<StyleId, Arc<ResolvedStyle>>;

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
                cell.value = Some(CellValue::DateTime(serial_to_date_time(serial)));
            }
        }
        cell.style = Some(resolved);
    }
    Ok(())
}

/// Excelのシリアル値（1900年うるう年バグを含む日付エポック）を
/// `DateTimeValue` へ変換する。具体的な変換式・エポック処理は
/// [model/cell.md オープンクエスチョン4](../model/cell.md) と連動して未確定。
fn serial_to_date_time(serial: f64) -> DateTimeValue {
    let _ = serial;
    unimplemented!()
}
```

## 依存関係

- 依存先: [`model/sheet.rs`](../model/sheet.md)（`Sheet::get_mut`, `CellRef`）、[`model/cell.rs`](../model/cell.md)（`CellValue`, `DateTimeValue`）、[`error.rs`](../error.md)
- 依存元: [`resolve/mod.rs`](mod.md)（`resolve_sheet` から呼び出される）。将来 `parse/styles.rs` が `StyleSheet` / `ResolvedStyle` / `StyleId` を構築する際、本ファイルが定義するこれらの型に依存する形になる見込み（オープンクエスチョン1参照）。

[`resolve/shared_strings.rs`](shared_strings.md) と同様、`sheet.get_mut` が `None` を返すケースは `parse/worksheet.rs` 側の不変条件違反（クレート内部のプログラミングエラー）とみなし `expect` を用いる（詳細はエラー処理方針参照）。

`CellValue::Number` から `CellValue::DateTime` への変換をスタイル適用と同一の関数内で行っている理由: numFmt（＝スタイル情報）を見なければ、あるセルの `f64` がプレーンな数値なのか日付シリアル値なのかを判別できないため、[model/cell.md](../model/cell.md) が既に述べる通りこの変換は本ファイル（`resolve/style.rs`）の責務である。[`resolve/shared_strings.rs`](shared_strings.md) の解決が本処理より先に走る前提（[resolve/mod.md](mod.md) の呼び出し順序）だが、`CellValue::Text` に対しては `is_date_time` を見ても変換を行わない（`if let Some(CellValue::Number(..))` によるパターンマッチで自然に除外される）ため、呼び出し順序が入れ替わっても本処理自体の正しさには影響しない。

## エラー処理方針

- `stylesheet.get(style_id)` が `None` を返す場合（`cellXfs` の範囲外インデックス）は `Error::InvalidStyleId` を返す。信頼できない外部入力に起因しうるため `panic` しない。
- `sheet.get_mut(entry.cell_ref)` が `None` を返すケースは [shared_strings.md エラー処理方針](shared_strings.md) と同じ理由（`parse/worksheet.rs` の不変条件違反）で `expect` によるパニックとする。
- 日付変換 `serial_to_date_time` 自体が失敗しうるか（例えば負のシリアル値、桁溢れ）は [model/cell.md オープンクエスチョン4](../model/cell.md) の型確定と合わせて未決定。現時点では `panic` しない実装（`Result` を返すか、境界値をクランプするか）を採用する方針で設計を進める（オープンクエスチョン2参照）。

## テスト方針

- 正当なスタイルIDを持つ `PendingStyle` が、`StyleSheet` の対応する `ResolvedStyle` で `Cell.style` に正しく設定されることの確認
- 存在しないスタイルID（`StyleSheet` の範囲外）を持つ場合に `Error::InvalidStyleId` を返すことの確認
- `is_date_time: true` なスタイルが `CellValue::Number` を持つセルに適用された場合、`CellValue::DateTime` へ変換されることの確認
- `is_date_time: true` なスタイルが `CellValue::Text` / `CellValue::Boolean` など数値以外を持つセルに適用された場合、`value` が変換されずそのまま保持されることの確認（誤変換防止の回帰テスト観点）
- `is_date_time: false` な通常のスタイルが適用された場合、`value` が変換されず `style` のみ設定されることの確認
- 同一の `ResolvedStyle` を複数セルへ適用した際、`Cell.style` の `Arc` が `Arc::ptr_eq` で同一（アロケーション重複がない）ことの確認（[model/cell.md](../model/cell.md) の `Arc` 設計方針との結線確認）
- `pending` が空リストの場合に何もせず `Ok(())` を返すことの確認

## 未決事項 / オープンクエスチョン

1. **`ResolvedStyle` / `StyleSheet` / `StyleId` の最終的な配置場所**: [model/mod.md オープンクエスチョン1](../model/mod.md) が「`model/` 側に置くか `resolve/style.rs` 側に置くかが未決定」としていた論点について、本ファイルでは暫定的に `resolve/style.rs` 側に定義する案を採用した。ただし `parse/styles.rs`（未設計）が `ResolvedStyle` を構築する主体になることを踏まえると、`parse/styles.rs` 側に定義を移す、あるいは両モジュールから参照される中立的な置き場所（例えば `model/style.rs` を新設する）が適切という判断もありうる。`parse/styles.rs` の設計時に最終確定する。
2. **日付/時刻書式の判定ロジックの置き場所**: 本ファイルは `ResolvedStyle.is_date_time` を既に判定済みの値として受け取る設計としたが、OOXMLの numFmt判定（組み込みID 14〜22等の範囲判定、カスタムフォーマット文字列のパターンマッチ）を `parse/styles.rs` 側で行うか、`resolve/style.rs` 側に判定ロジックそのものを持ち込むか（`ResolvedStyle` が生のフォーマット文字列を保持し、本ファイルが解釈する）は未決定。前者はarchitecture.md 設計方針2（`resolve/` はI/O非依存だが判定ロジック自体はドメイン知識でありどちらに置いても矛盾しない）を踏まえるとどちらでも成立するため、`parse/styles.rs` の設計時にあわせて確定させる。
3. **`serial_to_date_time` の実装**: [model/cell.md オープンクエスチョン4](../model/cell.md)（`DateTimeValue` の具体的な型、1900年うるう年バグの扱い）が解決してから実装する。関数シグネチャが `Result` を返すべきかどうかも本オープンクエスチョンに含む。
4. **フォント/塗りつぶし/罫線などの具体的なスタイル要素**: `ResolvedStyle` は現状 `is_date_time` のみを仮定義しているが、要求仕様書がセルスタイルとしてどこまでの要素（フォント色、背景色、罫線、太字/斜体等）をJSON出力に含める必要があるかは `json.rs` の設計、または要求仕様書自体の詳細化と合わせて確定させる。
