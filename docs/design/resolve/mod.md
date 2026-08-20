# `resolve/mod.rs` 設計書

*[English](mod.en.md)*

`src/resolve/mod.rs` に対応する設計書。[architecture.md](../architecture.md) が定義するフェーズ4（分析と遅延解決）のエントリポイントであり、[shared_strings.md](shared_strings.md) [merge.md](merge.md) [style.md](style.md) の各解決処理をシート単位でオーケストレーションする集約ファイル。architecture.md 設計方針2「`resolve/` 配下は I/O やXML構造に一切依存せず、`model::Sheet` などメモリ上のデータ構造のみで完結させる」を満たすことが本ファイルおよびサブモジュール全体の必須条件。

## 責務・スコープ

- サブモジュールの宣言（`mod shared_strings; mod merge; mod style; mod column_width; mod color;`）と公開型の再エクスポート
- 1シート分の未解決データ（フェーズ3が構築した `model::Sheet` と、共有文字列インデックス・スタイルIDの保留リスト、`<cols>` の範囲リストと `defaultColWidth`、`<mergeCells>` の範囲リスト）を受け取り、[shared_strings.md](shared_strings.md) → [style.md](style.md) → [column_width.md](column_width.md) → [merge.md](merge.md) の順で解決処理を呼び出すエントリ関数 `resolve_sheet` を提供する
- [`resolve/color.rs`](color.md)（`resolve_color`。Issue #76）を再エクスポートし、クレート外部から直接呼び出せるようにする——ただし [column_width.md](column_width.md) までの4つとは異なり `resolve_sheet` 自身からは呼び出さない（下記依存関係参照）
- **含まない責務**: 各解決処理そのもののロジック（共有文字列のインデックス引き当て、スタイルの適用、列幅範囲・結合範囲の検証・登録、色の解決は各サブモジュールの責務）、XMLパースそのもの（`parse/worksheet.rs` 等）、`SharedStringTable` / `StyleSheet` の構築（`parse/shared_strings.rs` / `parse/styles.rs`）

## 主要な型・関数（案）

```rust
mod shared_strings;
mod merge;
mod style;
mod column_width;
mod color;

pub use color::resolve_color;

use crate::error::Error;
use crate::model::sheet::{ColWidthRange, MergedRegion, Sheet};
use crate::model::style::StyleSheet;
// PendingSharedString/PendingStyleはフェーズ3の出力データそのものであるため
// parse/worksheet.rsが定義する（PR #9レビューを反映。依存関係セクション参照）。
use crate::parse::worksheet::{PendingSharedString, PendingStyle};

/// 1シート分の未解決データをまとめてフェーズ4の解決処理にかける。
/// `pipeline.rs` がシートごとに1回呼び出す想定のエントリ関数。
///
/// 呼び出し前提: `sheet` はフェーズ3（`parse/worksheet.rs`）によって
/// 全セルの挿入が完了している。ただし共有文字列参照セルは `value: None`
/// のまま、スタイル参照セルは `style: None` のまま挿入されている
/// （詳細は shared_strings.md / style.md 参照）。
pub fn resolve_sheet(
    sheet: &mut Sheet,
    pending_shared_strings: &[PendingSharedString],
    shared_string_table: &crate::parse::shared_strings::SharedStringTable,
    pending_styles: &[PendingStyle],
    stylesheet: &StyleSheet,
    col_width_ranges: Vec<ColWidthRange>,
    default_col_width: Option<f64>,
    merge_regions: Vec<MergedRegion>,
) -> Result<(), Error> {
    shared_strings::resolve(sheet, pending_shared_strings, shared_string_table)?;
    style::resolve(sheet, pending_styles, stylesheet)?;
    column_width::resolve(sheet, col_width_ranges, default_col_width)?;
    merge::resolve(sheet, merge_regions)?;
    Ok(())
}
```

## 依存関係

- 依存先: [`resolve/shared_strings.rs`](shared_strings.md), [`resolve/merge.rs`](merge.md), [`resolve/style.rs`](style.md), [`resolve/column_width.rs`](column_width.md), [`resolve/color.rs`](color.md)（すべて `mod` 宣言として）、[`model/sheet.rs`](../model/sheet.md)（`Sheet`, `MergedRegion`, `ColWidthRange`）、[`error.rs`](../error.md)。[`parse::shared_strings::SharedStringTable`](../parse/shared_strings.md)、[`parse::worksheet::{PendingSharedString, PendingStyle}`](../parse/worksheet.md) にも依存するが、これは architecture.md 設計方針2が禁じる「I/Oへの依存」ではなく「フェーズ3が既に構築済みの、メモリ上の構造化データへの依存」であるため、`resolve/` の I/O非依存方針とは矛盾しない（quick-xml や `std::fs` など実際のI/O・XML構造への依存は持たない）。
- 依存元: `pipeline.rs`（各シートのフェーズ3完了後に `resolve_sheet` を呼び出す）、クレート外部の呼び出し元（再エクスポートされた `resolve_color` を、`Workbook::theme()`/`ResolvedStyle.fill_fg_color`等と組み合わせて任意のタイミングで直接呼び出す。Issue #76「案A」）

`resolve_color`(Issue #76)を `resolve_sheet` から呼び出さない理由: [resolve/color.md](color.md) が採用する「案A: オンデマンド解決API」は、色解決を全セル走査(フェーズ4)から独立させ、呼び出し側が表示用途で実際に必要とした箇所でのみ計算することでセル数に対するCPU・メモリオーバーヘッドを避ける設計であるため([Issue #76 設計提案](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)参照)。`shared_strings`/`style`/`column_width`/`merge` の4つがいずれも「フェーズ3が残した保留状態を解消しない限りセルが不完全なままになる」必須処理であるのに対し、色解決は`ColorRef`という完全な情報が既に`ResolvedStyle`に格納された*後*に、必要な場合のみ行う付加的な変換である点が異なる。

共有文字列解決・スタイル適用・列幅解決の3つには強い前後関係はない（それぞれが読み書きする状態が独立しているため）。結合解決を最後に置いているのは保険的な順序である: [merge.md](merge.md) の `insert_merge` は呼び出し時点で起点セルが `cells` に存在することを前提としており、さらにIssue #43以降は `merge::resolve` の最終ステップとして呼ばれる `Sheet::finalize_merges` が `cells` から起点以外の全エントリを削除するため、仮想(非起点)座標にまだ触れる可能性のある他の全ステップが完了した後でなければならない。

## エラー処理方針

- `resolve_sheet` は3つのサブ処理を `?` で早期リターンする。いずれか1つが失敗した場合、後続の解決処理は実行されない（例えば共有文字列解決が失敗した場合、スタイル適用・結合解決は行わずに中断する）。部分的に解決されたシートを呼び出し元に返さないことで、不完全な状態のデータがJSON生成まで到達することを防ぐ（fail closed）。
- 各サブモジュールが返すエラー種別（`Error::SharedStringIndexOutOfBounds` / `Error::InvalidStyleId` / `Error::InvalidMergedRange`）はそのまま呼び出し元（`pipeline.rs`）へ伝播する。本ファイル自身は新たなエラーバリアントを生成しない。

## テスト方針

- 3つのサブ処理すべてが正常に完了する最小ケース（共有文字列参照セル1件、スタイル参照セル1件、結合範囲1件を含むシート）で `resolve_sheet` が `Ok(())` を返し、各セルが期待通り解決済みになることの確認（結合テスト。各サブ処理のロジック自体の網羅的なテストは各サブモジュールの責務）
- 共有文字列解決が失敗する場合（範囲外インデックス）に、後続のスタイル適用・結合解決が実行されずに即座にエラーが伝播することの確認
- 保留リスト・結合範囲リストがいずれも空（プレーンな数値・真偽値セルのみのシート）の場合に `resolve_sheet` が何もせず `Ok(())` を返すことの確認

## 未決事項 / オープンクエスチョン

1. ~~`SharedStringTable` の構築元モジュール~~ → **解決**: [`parse/shared_strings.rs`](../parse/shared_strings.md) が `SharedStringTable` を定義・構築する。`StyleSheet` / `ResolvedStyle` / `StyleId` は [`model/style.rs`](../model/style.md) 側で先行して定義済み（PR #8 レビュー指摘を反映）であり、[`parse/styles.rs`](../parse/styles.md) 設計時に整合を確認済み。
2. **サブ処理間の実行順序の妥当性**: 現状「共有文字列解決 → スタイル適用 → 結合解決」の順に強い技術的根拠はなく、将来的にスタイル適用時のnumFmt日付判定が共有文字列解決結果（`CellValue::Text`）を誤って上書きしないことをテストで担保する必要がある程度の緩い依存しかない。並行実行（`sheet` への同時可変アクセスが必要なため現状の設計では不可）にする価値があるかは、実装時のプロファイリング結果を踏まえて再検討する。
3. ~~`pending_shared_strings` / `pending_styles` の受け渡し方法~~ → **解決**: [`parse/worksheet.rs`](../parse/worksheet.md) が `Vec` としてまとめて構築し、`resolve_sheet` へそのまま渡す設計とした（「フェーズ3完了後にフェーズ4を実行する」という architecture.md の一方向パイプライン方針どおり）。`PendingSharedString`/`PendingStyle` 自体の型定義は [`parse/worksheet.rs`](../parse/worksheet.md) 側へ移設され（[PR #9 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204)を反映）、本ファイル・[`resolve/shared_strings.rs`](shared_strings.md)・[`resolve/style.rs`](style.md) はいずれもそれを `use` する側に統一された（[parse/worksheet.md オープンクエスチョン1](../parse/worksheet.md) を解決）。
