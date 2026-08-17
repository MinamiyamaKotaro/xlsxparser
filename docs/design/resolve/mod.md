# `resolve/mod.rs` 設計書

*[English](mod.en.md)*

`src/resolve/mod.rs` に対応する設計書。[architecture.md](../architecture.md) が定義するフェーズ4（分析と遅延解決）のエントリポイントであり、[shared_strings.md](shared_strings.md) [merge.md](merge.md) [style.md](style.md) の各解決処理をシート単位でオーケストレーションする集約ファイル。architecture.md 設計方針2「`resolve/` 配下は I/O やXML構造に一切依存せず、`model::Sheet` などメモリ上のデータ構造のみで完結させる」を満たすことが本ファイルおよびサブモジュール全体の必須条件。

## 責務・スコープ

- サブモジュールの宣言（`mod shared_strings; mod merge; mod style;`）と公開型の再エクスポート
- 1シート分の未解決データ（フェーズ3が構築した `model::Sheet` と、共有文字列インデックス・スタイルIDの保留リスト、`<mergeCells>` の範囲リスト）を受け取り、[shared_strings.md](shared_strings.md) → [style.md](style.md) → [merge.md](merge.md) の順で解決処理を呼び出すエントリ関数 `resolve_sheet` を提供する
- **含まない責務**: 各解決処理そのもののロジック（共有文字列のインデックス引き当て、スタイルの適用、結合範囲の検証・登録は各サブモジュールの責務）、XMLパースそのもの（`parse/worksheet.rs` 等）、`SharedStringTable` / `StyleSheet` の構築（`parse/shared_strings.rs` / `parse/styles.rs`。未設計、オープンクエスチョン1参照）

## 主要な型・関数（案）

```rust
mod shared_strings;
mod merge;
mod style;

pub use shared_strings::PendingSharedString;
pub use style::{PendingStyle, ResolvedStyle, StyleId, StyleSheet};

use crate::error::Error;
use crate::model::sheet::{MergedRegion, Sheet};

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
    merge_regions: Vec<MergedRegion>,
) -> Result<(), Error> {
    shared_strings::resolve(sheet, pending_shared_strings, shared_string_table)?;
    style::resolve(sheet, pending_styles, stylesheet)?;
    merge::resolve(sheet, merge_regions)?;
    Ok(())
}
```

## 依存関係

- 依存先: [`resolve/shared_strings.rs`](shared_strings.md), [`resolve/merge.rs`](merge.md), [`resolve/style.rs`](style.md)（すべて `mod` 宣言として）、[`model/sheet.rs`](../model/sheet.md)（`Sheet`, `MergedRegion`）、[`error.rs`](../error.md)。`parse::shared_strings::SharedStringTable`（未設計、オープンクエスチョン1参照）にも依存するが、これは architecture.md 設計方針2が禁じる「I/Oへの依存」ではなく「フェーズ3が既に構築済みの、メモリ上の構造化データへの依存」であるため、`resolve/` の I/O非依存方針とは矛盾しない（quick-xml や `std::fs` など実際のI/O・XML構造への依存は持たない）。
- 依存元: `pipeline.rs`（各シートのフェーズ3完了後に `resolve_sheet` を呼び出す）

`resolve_sheet` 内の呼び出し順序（共有文字列解決 → スタイル適用 → 結合解決）に強い前後関係はない（各サブモジュールが読み書きするセルのフィールドが独立しているため）。結合解決を最後に置いているのは、[merge.md](merge.md) の `insert_merge` が呼び出し時点で起点セルが `cells` に存在することを前提とするための保険的な順序であり、共有文字列・スタイルの解決漏れがあった場合に問題を早期に検出しやすくする意図（詳細はオープンクエスチョン2参照）。

## エラー処理方針

- `resolve_sheet` は3つのサブ処理を `?` で早期リターンする。いずれか1つが失敗した場合、後続の解決処理は実行されない（例えば共有文字列解決が失敗した場合、スタイル適用・結合解決は行わずに中断する）。部分的に解決されたシートを呼び出し元に返さないことで、不完全な状態のデータがJSON生成まで到達することを防ぐ（fail closed）。
- 各サブモジュールが返すエラー種別（`Error::SharedStringIndexOutOfBounds` / `Error::InvalidStyleId` / `Error::InvalidMergedRange`）はそのまま呼び出し元（`pipeline.rs`）へ伝播する。本ファイル自身は新たなエラーバリアントを生成しない。

## テスト方針

- 3つのサブ処理すべてが正常に完了する最小ケース（共有文字列参照セル1件、スタイル参照セル1件、結合範囲1件を含むシート）で `resolve_sheet` が `Ok(())` を返し、各セルが期待通り解決済みになることの確認（結合テスト。各サブ処理のロジック自体の網羅的なテストは各サブモジュールの責務）
- 共有文字列解決が失敗する場合（範囲外インデックス）に、後続のスタイル適用・結合解決が実行されずに即座にエラーが伝播することの確認
- 保留リスト・結合範囲リストがいずれも空（プレーンな数値・真偽値セルのみのシート）の場合に `resolve_sheet` が何もせず `Ok(())` を返すことの確認

## 未決事項 / オープンクエスチョン

1. **`SharedStringTable` / `StyleSheet` の構築元モジュール**: `parse/shared_strings.rs` / `parse/styles.rs` は本Issueのスコープ外（architecture.md 記載の予定モジュールのみ）でまだ設計されていない。`SharedStringTable` の具体的な型・配置場所（`parse::shared_strings` か `resolve::shared_strings` か）は `parse/shared_strings.rs` の設計時に確定させる。`StyleSheet` / `ResolvedStyle` / `StyleId` は本ドキュメント群（[style.md](style.md)）側で先行して定義したが、`parse/styles.rs` 設計時に整合を再確認する。
2. **サブ処理間の実行順序の妥当性**: 現状「共有文字列解決 → スタイル適用 → 結合解決」の順に強い技術的根拠はなく、将来的にスタイル適用時のnumFmt日付判定が共有文字列解決結果（`CellValue::Text`）を誤って上書きしないことをテストで担保する必要がある程度の緩い依存しかない。並行実行（`sheet` への同時可変アクセスが必要なため現状の設計では不可）にする価値があるかは、実装時のプロファイリング結果を踏まえて再検討する。
3. **`pending_shared_strings` / `pending_styles` の受け渡し方法**: `parse/worksheet.rs` が未設計のため、これらのリストが `Vec` としてまとめて受け渡されるのか、`Sheet` 構築とインターリーブしたストリーミング処理の一部として逐次解決されるのかは未確定。現状は「フェーズ3完了後にフェーズ4を実行する」という architecture.md の一方向パイプライン方針に従い、`Vec` としてまとめて受け渡す設計を仮定している。
