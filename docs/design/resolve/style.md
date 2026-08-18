# `resolve/style.rs` 設計書

*[English](style.en.md)*

`src/resolve/style.rs` に対応する設計書。[architecture.md](../architecture.md) が定義するフェーズ4の一部「セルスタイルの適用」を担う。[`model/style.rs`](../model/style.md) が定義する `StyleSheet` から得たスタイル定義を各セルへ適用し、あわせて数値書式（numFmt）が日付/時刻を表す場合の `CellValue::Number` → `CellValue::DateTime` 変換を行う。

## 責務・スコープ

- フェーズ3（`parse/worksheet.rs`）が記録した「スタイルID参照セルの保留リスト」を受け取り、[`model::style::StyleSheet`](../model/style.md) を引いて解決済みスタイル（`ResolvedStyle`）を各セルの `style: Option<Arc<ResolvedStyle>>` へ設定する
- 適用したスタイルの数値書式（numFmt）が日付/時刻書式であると判定した場合、対象セルの `CellValue::Number` を `CellValue::DateTime` へ変換する。変換できない値（負値・`NaN`・`Infinity`・Excelの表現範囲外など）の場合は変換をスキップし、`CellValue::Number` を維持したまま処理を継続する（フォールバック。PR #8 レビュー指摘を反映してオープンクエスチョン3を解決。詳細はエラー処理方針参照）
- `serial_to_date_time` によるシリアル値→暦への実際の分解を行う(Issue #40)。`[parse/workbook.rs](../parse/workbook.md)` が読み取った `date1904: bool`(`<workbookPr date1904="1"/>`)を `resolve()` の引数として受け取り、1900日付システムと1904日付システムいずれのエポックを使うかを選択する
- スタイルIDがスタイル定義の範囲外の場合に `Error::InvalidStyleId` を返す
- **含まない責務**: `styles.xml` のXMLパースおよび `ResolvedStyle` を構築するロジックそのもの（`parse/styles.rs`。本ファイルは構築済みの `StyleSheet` を受け取る前提とする）、`ResolvedStyle` / `StyleSheet` / `StyleId` の型定義そのもの（[`model/style.rs`](../model/style.md) へ移動。PR #8 レビュー指摘を反映してオープンクエスチョン1を解決）、日付/時刻書式かどうかの具体的な numFmt コード判定ルール自体の実装（本ファイルは判定結果を `ResolvedStyle` が既に保持している前提とする。オープンクエスチョン2参照）、`date1904` フラグそのものの読み取り([`parse/workbook.rs`](../parse/workbook.md)——本ファイルは受け取った値を使うのみ)

## 主要な型・関数（案）

```rust
use crate::error::Error;
use crate::model::cell::{CellValue, DateTimeValue};
use crate::model::sheet::Sheet;
use crate::model::style::{ResolvedStyle, StyleSheet};
// PendingStyleはフェーズ3の出力データそのものであるため
// parse/worksheet.rsが定義する（PR #9レビューを反映。依存関係セクション参照）。
use crate::parse::worksheet::PendingStyle;

/// `pending` の各エントリについて `stylesheet` から `ResolvedStyle` を引き、
/// `sheet` の対応セルへ設定する。あわせて `is_date_time` な書式が
/// `CellValue::Number` に適用された場合、`CellValue::DateTime` へ変換する。
/// `date1904`(`<workbookPr date1904="1"/>`。フェーズ1で一度だけ読み取り済み)
/// は、Excelの2つのシリアル値エポックのどちらを使うかを選択する
/// (Issue #40。`serial_to_date_time` 参照)。
pub(crate) fn resolve(
    sheet: &mut Sheet,
    pending: &[PendingStyle],
    stylesheet: &StyleSheet,
    date1904: bool,
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
                if let Some(dt) = serial_to_date_time(serial, date1904) {
                    cell.value = Some(CellValue::DateTime(dt));
                }
            }
        }
        cell.style = Some(resolved);
    }
    Ok(())
}

/// Excelのシリアル値を `DateTimeValue` へ変換する。`date1904` に応じて
/// 1900日付システム(エポック相当 1899-12-30)・1904日付システム(エポック
/// 1904-01-01)いずれかを使う。負値・`NaN`・`Infinity`・Excelの表現範囲
/// （最大 9999年12月31日相当）を超える値など、変換不能な値に対しては
/// `None` を返す（エラーにはしない。エラー処理方針参照）。
///
/// **1900年うるう年バグ**: 1900日付システムでは、単純なエポックオフセット
/// 演算だけでは正しい結果にならない——検証の結果判明したことだが、
/// シリアル値1〜59に対しては素朴なグレゴリオ暦演算は実際のExcel挙動より
/// 1日早い日付を返し（例: シリアル1は本来1900-01-01だが素朴な演算では
/// 1899-12-31になる）、シリアル60（Excel自身が「1900年2月29日」と文書化
/// している架空の日、Microsoft KB214326）に対応する実在の暦日は
/// 存在しない（1900年は実際には閏年ではない）。そのため、シリアル値
/// 1〜59は+1日シフトしてから変換し（この技法はopenpyxl等多くの
/// Excel互換リーダーが採用するものと同じ）、シリアル60自体は変換不能
/// として直接ハードコードする。日付部分の変換は Howard Hinnant の
/// `civil_from_days` アルゴリズム(パブリックドメイン、整数演算のみ、
/// `chrono` 等の外部日付クレートに依存しない——Issue #40が要求する
/// パフォーマンス面の考慮)で行う。1904日付システムには閏年バグは
/// 存在しない(1904年は実在の閏年のため)。
fn serial_to_date_time(serial: f64, date1904: bool) -> Option<DateTimeValue> {
    let _ = (serial, date1904);
    unimplemented!()
}
```

## 依存関係

- 依存先: [`model/sheet.rs`](../model/sheet.md)（`Sheet::get_mut`）、[`model/cell.rs`](../model/cell.md)（`CellValue`, `DateTimeValue`）、[`model/style.rs`](../model/style.md)（`ResolvedStyle`, `StyleSheet`。PR #8 レビュー指摘を反映し本ファイルから移動）、[`error.rs`](../error.md)、[`parse::worksheet::PendingStyle`](../parse/worksheet.md)
- 依存元: [`resolve/mod.rs`](mod.md)（`resolve_sheet` から呼び出される。`date1904` は [`pipeline.rs`](../pipeline.md) がフェーズ1で `[parse/workbook.rs](../parse/workbook.md)` から一度だけ読み取り、`model::Workbook` 自体には保持せず `resolve_sheet` 呼び出しへそのまま引き渡す——[model/style.md](../model/style.md) の `StyleSheet` と同じ「フェーズ間の一時値」扱い）

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
- **シリアル値1が1900日付システムで「1900-01-01」に解決されることの確認**——素朴なエポックオフセット演算のみだと「1899-12-31」になってしまう回帰テスト(Issue #40)
- **シリアル値59・61がそれぞれ「1900-02-28」「1900-03-01」に解決され、シリアル値60が架空の「1900-02-29」(Microsoft KB214326)に解決されることの確認**——1900年うるう年バグの境界値テスト
- **1904日付システム(`date1904: true`)ではシリアル値60が通常の(架空でない)日付に解決されることの確認**——1904年は実在の閏年のため、うるう年バグが存在しないことの確認
- **小数部から時刻(時/分/秒)が正しく分解されることの確認**、および小数部の丸めがちょうど86,400秒(1日全体)になった場合に次の日へ繰り上がることの確認(浮動小数点丸め誤差の境界値テスト)

## 未決事項 / オープンクエスチョン

1. ~~`ResolvedStyle` / `StyleSheet` / `StyleId` の最終的な配置場所~~ → **解決**: [`model/style.rs`](../model/style.md) を新設し、そちらに定義する。`parse/styles.rs`（構築主体）と `resolve/style.rs`（適用主体）の双方が `model/` にのみ依存する構造とすることで、レイヤー間の独立性を保つ（PR #8 レビュー指摘を反映）。
2. ~~日付/時刻書式の判定ロジックの置き場所~~ → **解決**: [`parse/styles.rs`](../parse/styles.md) 側で判定する（OOXMLの numFmt判定を含む）。本ファイルは引き続き `ResolvedStyle.is_date_time` を既に判定済みの値として受け取るのみで、判定ロジックそのものは持たない。判定ヒューリスティックの精度自体は [parse/styles.md オープンクエスチョン2](../parse/styles.md) として引き続き未解決。
3. ~~`serial_to_date_time` の実装~~ → **解決**(Issue #40): 変換不能な値に対しては `Error` を返さず `None` を返し、呼び出し側（本ファイルの `resolve`）が `CellValue::Number` を維持するフォールバックとする方針(PR #8 レビュー指摘を反映)に加え、変換式自体(1900年うるう年バグ・1904日付システムの扱いを含む)も上記の通り実装済み。設計時に「エポックオフセットだけでうるう年バグを自動的に吸収できる」と想定していたが、実装時の検証(具体的な日付での逆算)でこれが誤りだと判明し、シリアル値1〜59への+1日シフトとシリアル60のハードコードという明示的な補正が必要だと分かった——「測定・検証してから確定する」という本プロジェクトの一貫した方針([Issue #43](https://github.com/MinamiyamaKotaro/xlsxparser/issues/43)のパフォーマンス調査時と同じ姿勢)を、日付変換の正しさの検証にも適用した結果。
4. **フォント/塗りつぶし/罫線などの具体的なスタイル要素**: [model/style.md オープンクエスチョン1](../model/style.md) と同一の論点（未解決）。要求仕様書がセルスタイルとしてどこまでの要素をJSON出力に含める必要があるかは `json.rs` の設計、または要求仕様書自体の詳細化と合わせて確定させる。
