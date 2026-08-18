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

/// 解決済みの `<font>` エントリ。Issue #38 が必要とする2つのプロパティ
/// のみを持ち、`CT_Font` の完全な転写ではない(色・フォント名・斜体・
/// 下線などは、具体的な用途が現れるまでスコープ外。オープンクエスチョン1参照)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Font {
    pub size_pt: f64,
    pub bold: bool,
}

impl Default for Font {
    /// Excel自身の既定値(「標準」スタイル、"Calibri 11"、太字なし)——
    /// `<xf>` の `fontId` が欠落・解決不能な場合に `parse/styles.rs` が
    /// 使うフォールバック。欠落・不正な `numFmtId` に対して既に確立
    /// されている `is_date_time` の段階的縮退方針と同じ考え方。
    fn default() -> Self {
        Font { size_pt: 11.0, bold: false }
    }
}

/// `<xf><alignment horizontal=".."/></xf>` の水平方向配置
/// (ECMA-376 `ST_HorizontalAlignmentValues`)、Issue #42。文字列ではなく
/// 列挙型として持つことで、コピー可能な小サイズに収める(Issue #42の
/// パフォーマンス要件)。垂直方向配置や `wrapText`/`horizontal` 以外の
/// その他の `CT_CellAlignment` 属性は、具体的な下流ユースケースが現れる
/// までスコープ外のまま(`Font` が既に採用している「完全な転写はしない」
/// 方針と同じ)。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Alignment {
    #[default]
    General,
    Left,
    Center,
    Right,
    Fill,
    Justify,
    CenterContinuous,
    Distributed,
}

/// スタイルID解決後の書式情報。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolvedStyle {
    /// この書式が日付/時刻を表すか。`parse/styles.rs` が `numFmts` の
    /// コード文字列（組み込み・カスタム双方）を解釈し、あらかじめ
    /// 判定した結果をここに格納しておく想定（[resolve/style.md オープンクエスチョン2](../resolve/style.md) 参照）。
    pub is_date_time: bool,
    pub font: Font,
    /// `<cellXfs><xf><alignment wrapText="1"/></xf></cellXfs>`(Issue #37)
    /// ——下流の方眼紙判定ツールがはみ出し判定のゲート条件として使う:
    /// 折返し設定のセルは決してはみ出し扱いにならない。
    pub wrap_text: bool,
    /// `<cellXfs><xf><alignment horizontal=".."/></xf></cellXfs>`
    /// (Issue #42)。`alignment` ではなく `horizontal_` を冠する名前に
    /// することで、将来 `vertical_alignment` を追加する際にフィールド名の
    /// 衝突・改名を避ける。
    pub horizontal_alignment: Alignment,
    /// `<xf>` が参照する `numFmtId` を書式コード文字列へ解決したもの
    /// (Issue #41)——組み込み(ECMA-376 Part 1 §18.8.30)・カスタム
    /// (`<numFmts>`)いずれも対象。`None` は `numFmtId=0`(「General」)、
    /// `numFmtId` 属性の欠落、いずれの表にも見つからないIDのいずれかを
    /// 表す——「General」は「特別な書式なし」以上の情報を持たないため、
    /// `Some("General")` ではなく「見つからない」場合と同じ扱いとする。
    /// `Arc<str>` を採用する理由は `CellValue::Text` と同じ: 同一の
    /// 書式コードが多数の `StyleId` 間で共有されることが多いため。
    pub number_format: Option<Arc<str>>,
    // fill/border等その他のプロパティは、各サブIssueの実装が進むにつれて
    // 追加する(オープンクエスチョン1参照)。
}

/// `cellXfs` インデックスから `ResolvedStyle` を引くテーブル。
/// `parse/styles.rs` が構築する想定。
pub type StyleSheet = HashMap<StyleId, Arc<ResolvedStyle>>;
```

`ResolvedStyle`/`Font` はいずれも `Default` を導出/実装しているため、一部のフィールドしか必要としない呼び出し元(ほとんどのテストフィクスチャ)は全フィールドを列挙せず `ResolvedStyle { is_date_time: true, ..Default::default() }` のように書ける。

## 依存関係

- 依存先: なし（`model/` 内の兄弟モジュールにも依存しない、リーフモジュール。[`model/cell.rs`](cell.md) と同様の位置づけ）
- 依存元: [`model/cell.rs`](cell.md)（`Cell.style: Option<Arc<ResolvedStyle>>` として参照）、[`resolve/style.rs`](../resolve/style.md)（`StyleSheet` を引いて `ResolvedStyle` を適用する）、`parse/styles.rs`（未設計。`StyleSheet` を構築する主体になる見込み）

`resolve/` と `parse/` の双方が本ファイル（`model/`）にのみ依存し、互いには直接依存しない構造とすることで、[architecture.md](../architecture.md) 設計方針2（I/O層とドメインロジックの分離）を保ったまま、フェーズ3とフェーズ4の間で型を安全に受け渡せる（PR #8 レビュー指摘を反映）。

## エラー処理方針

対象なし（[`model/cell.rs`](cell.md) 同様、ロジックを持たない純粋データ構造の定義のみ）。存在しないスタイルIDの参照（`StyleSheet::get` が `None` を返すケース）に対するエラー化は [`resolve/style.rs`](../resolve/style.md) の責務。

## テスト方針

対象なし。型定義のみのためユニットテストを持たない。`ResolvedStyle` の等価性・`Arc` 共有の挙動検証は [resolve/style.md](../resolve/style.md) のテスト方針側で行う。

## 未決事項 / オープンクエスチョン

1. **塗りつぶし/罫線/折返し/配置などの具体的なスタイル要素**: さらに解決が進んだ——`font: Font { size_pt, bold }`(Issue #38)、`wrap_text: bool`(Issue #37、はみ出し判定のゲート条件)、`number_format: Option<Arc<str>>`(Issue #41)、`horizontal_alignment: Alignment`(Issue #42)をいずれも実装済み。[Issue #36](https://github.com/MinamiyamaKotaro/xlsxparser/issues/36) 配下のサブIssueは全て解決済み。フォント色・塗りつぶし・罫線・斜体・下線などその他の `CT_Font`/`CT_Fill`/`CT_Border` プロパティ、および `wrapText`/`horizontal` 以外の `CT_CellAlignment` の属性(垂直方向配置、インデント、テキスト回転等)は、具体的な下流ユースケースが現れるまでスコープ外のまま(`Font` が既に採用している「完全な転写はしない」方針と同じ)。
2. ~~日付/時刻書式の判定ロジックの置き場所~~ → **解決**: [`parse/styles.rs`](../parse/styles.md) が `numFmtId`/`formatCode` から `ResolvedStyle::is_date_time` を判定するロジックを持つ（[resolve/style.md オープンクエスチョン2](../resolve/style.md) と同一の論点）。判定ヒューリスティックの精度自体は [parse/styles.md オープンクエスチョン2](../parse/styles.md) として引き続き未解決。
