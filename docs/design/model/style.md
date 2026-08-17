# `model/style.rs` 設計書

*[English](style.en.md)*

`src/model/style.rs` に対応する設計書。[model/mod.md オープンクエスチョン1](mod.md) および [model/cell.md](cell.md) が「`model/` 側に置くか `resolve/style.rs` 側に置くかが未決定」としていた `ResolvedStyle` の置き場所を解決するために新設するファイル（PR #8 レビュー指摘を反映）。セルスタイルの解決結果を表す、ロジックを持たない純粋なデータ構造のみを定義する。

`parse/styles.rs`（未設計。`styles.xml` から `ResolvedStyle` を構築する主体）と [`resolve/style.rs`](../resolve/style.md)（構築済みの `ResolvedStyle` をセルへ適用する主体）が、互いを直接知ることなく本ファイルの型だけを介して間接的につながる、フェーズ3・フェーズ4間の共有語彙として機能する。[`model/cell.rs`](cell.md) の `Cell` / `Sheet` が `parse/` と `resolve/` の双方から参照される共有データ構造であるのと同じ位置づけ。

## 責務・スコープ

- `cellXfs` インデックス（スタイルID）の型 `StyleId` を定義する
- スタイルID解決後の書式情報 `ResolvedStyle` を定義する
- `cellXfs` インデックスから `ResolvedStyle` を引くテーブル型 `StyleSheet` を定義する
- **含まない責務**: `styles.xml` のXMLパースや `ResolvedStyle` の構築ロジックそのもの（`parse/styles.rs`、未設計）、`ResolvedStyle` をセルへ適用する処理そのもの（[`resolve/style.rs`](../resolve/style.md)）、日付/時刻書式かどうかの具体的な numFmt コード判定ルール自体の実装（[resolve/style.md オープンクエスチョン2](../resolve/style.md) 参照。本ファイルは判定結果を保持するフィールドのみを定義する）

## 主要な型（案）

```rust
use std::collections::HashMap;
use std::sync::Arc;

/// `cellXfs` のインデックス（スタイルID）。[error.rs](../error.md) の
/// `Error::InvalidStyleId(u32)` と型を揃える。
pub type StyleId = u32;

/// スタイルID解決後の書式情報。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedStyle {
    /// この書式が日付/時刻を表すか。`parse/styles.rs` が `numFmts` の
    /// コード文字列（組み込み・カスタム双方）を解釈し、あらかじめ
    /// 判定した結果をここに格納しておく想定（[resolve/style.md オープンクエスチョン2](../resolve/style.md) 参照）。
    pub is_date_time: bool,
    // font/fill/border 等の具体的なフィールドは parse/styles.rs の設計時に確定させる
    // （オープンクエスチョン1参照）。
}

/// `cellXfs` インデックスから `ResolvedStyle` を引くテーブル。
/// `parse/styles.rs` が構築する想定。
pub type StyleSheet = HashMap<StyleId, Arc<ResolvedStyle>>;
```

## 依存関係

- 依存先: なし（`model/` 内の兄弟モジュールにも依存しない、リーフモジュール。[`model/cell.rs`](cell.md) と同様の位置づけ）
- 依存元: [`model/cell.rs`](cell.md)（`Cell.style: Option<Arc<ResolvedStyle>>` として参照）、[`resolve/style.rs`](../resolve/style.md)（`StyleSheet` を引いて `ResolvedStyle` を適用する）、`parse/styles.rs`（未設計。`StyleSheet` を構築する主体になる見込み）

`resolve/` と `parse/` の双方が本ファイル（`model/`）にのみ依存し、互いには直接依存しない構造とすることで、[architecture.md](../architecture.md) 設計方針2（I/O層とドメインロジックの分離）を保ったまま、フェーズ3とフェーズ4の間で型を安全に受け渡せる（PR #8 レビュー指摘を反映）。

## エラー処理方針

対象なし（[`model/cell.rs`](cell.md) 同様、ロジックを持たない純粋データ構造の定義のみ）。存在しないスタイルIDの参照（`StyleSheet::get` が `None` を返すケース）に対するエラー化は [`resolve/style.rs`](../resolve/style.md) の責務。

## テスト方針

対象なし。型定義のみのためユニットテストを持たない。`ResolvedStyle` の等価性・`Arc` 共有の挙動検証は [resolve/style.md](../resolve/style.md) のテスト方針側で行う。

## 未決事項 / オープンクエスチョン

1. **フォント/塗りつぶし/罫線などの具体的なスタイル要素**: [resolve/style.md オープンクエスチョン4](../resolve/style.md) と同一の論点。`ResolvedStyle` は現状 `is_date_time` のみを仮定義しているが、要求仕様書がセルスタイルとしてどこまでの要素（フォント色、背景色、罫線、太字/斜体等）をJSON出力に含める必要があるかは `json.rs` の設計、または要求仕様書自体の詳細化と合わせて確定させる。
2. ~~日付/時刻書式の判定ロジックの置き場所~~ → **解決**: [`parse/styles.rs`](../parse/styles.md) が `numFmtId`/`formatCode` から `ResolvedStyle::is_date_time` を判定するロジックを持つ（[resolve/style.md オープンクエスチョン2](../resolve/style.md) と同一の論点）。判定ヒューリスティックの精度自体は [parse/styles.md オープンクエスチョン2](../parse/styles.md) として引き続き未解決。
