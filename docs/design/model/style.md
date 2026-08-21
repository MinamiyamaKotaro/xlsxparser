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

/// セルの各辺に罫線があるかどうか(Issue #97)——線種・太さ・色は
/// 対象外、`Font` が既に採用している「完全な転写はしない」方針と同じ:
/// この対応の動機になった方眼紙判定ユースケースは、セルが枠で
/// 囲まれている**かどうか**だけが必要で、どのような線かは不要。
/// `<diagonal>`(対角線)は対象外——方眼紙判定に必要という要求が無く、
/// Excel自身のUIでもほとんど使われない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Borders {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl Borders {
    /// いずれかの辺に罫線があるか——`json.rs`がこれを使って`borders`
    /// オブジェクト自体を出力するかどうかを決める。`col_width_ranges`/
    /// `fill_fg_color`と同じ疎な出力の原則(ほとんどのセルはどの辺にも
    /// 罫線を持たない)。
    pub fn any(&self) -> bool {
        self.top || self.right || self.bottom || self.left
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
    /// `<fill><patternFill><fgColor .../></patternFill></fill>`
    /// (Issue #75)、生の指定のまま——下記の`ColorRef`参照。
    pub fill_fg_color: Option<ColorRef>,
    /// `fill_fg_color`と同様、`<bgColor>`用。
    pub fill_bg_color: Option<ColorRef>,
    /// `<xf borderId="..">`を`<borders>`に対して解決したもの(Issue
    /// #97)、辺ごとの有無のみ。(`font: Font`と同様に)4つのトップ
    /// レベルフィールドへ展開せずネストする——`Borders`の4つの真偽値は
    /// `Font`の`size_pt`/`bold`と同じく自然に1つの概念としてまとまる。
    /// `fill_fg_color`/`fill_bg_color`が分割されているのは、それぞれを
    /// 独立した`Option<ColorRef>`として個別にdiffできるようにするための
    /// 別の理由による。
    pub borders: Borders,
}

/// セル塗りつぶしの前景色/背景色を、`<fgColor>`/`<bgColor>`が指定する
/// そのままの形で保持する(Issue #75)——最終的な表示RGB値へは変換
/// しない。xlsxparserの出力はレンダリング用ではなくdiff用途であり、
/// 「塗りつぶし色が変わったこと」の検出には`ColorRef`同士を直接比較
/// (`PartialEq`)すれば十分で、実際に何色として表示されるかを知る
/// 必要はない。`Theme`/`Indexed`を実RGB値へ解決するのは別の
/// 表示用途の関心事(Issue #76)。
#[derive(Debug, Clone, PartialEq)]
pub enum ColorRef {
    /// `rgb="FFFF0000"`、そのまま保持。`Arc<str>`を使う理由は
    /// `number_format`と同じ——多数の`StyleId`が同一`fillId`を
    /// 共有することが多いため。
    Rgb(Arc<str>),
    /// `theme="4" tint="-0.25"`——ワークブックの`theme{N}.xml`の
    /// `<clrScheme>`へのインデックスと、任意の輝度補正値。`tint`は
    /// `theme{N}.xml`自体には存在せず、参照側に個別に付与される
    /// ため、`None`は「`tint`属性が全く無い」ことを表す(明示的な
    /// `tint="0"`とは区別される)。
    Theme { index: u32, tint: Option<f64> },
    /// `indexed="64"`——OOXML以前のレガシーな64色パレットへの
    /// インデックス。
    Indexed(u32),
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

1. **塗りつぶし/罫線/折返し/配置などの具体的なスタイル要素**: さらに解決が進んだ——`font: Font { size_pt, bold }`(Issue #38)、`wrap_text: bool`(Issue #37、はみ出し判定のゲート条件)、`number_format: Option<Arc<str>>`(Issue #41)、`horizontal_alignment: Alignment`(Issue #42)、`fill_fg_color`/`fill_bg_color: Option<ColorRef>`(Issue #75)、`borders: Borders`(Issue #97、辺ごとの有無のみ)をいずれも実装済み。[Issue #36](https://github.com/MinamiyamaKotaro/xlsxparser/issues/36) 配下のサブIssueに加え、派生の塗りつぶし色・罫線Issueも解決済み。`ColorRef`は実RGB値ではなく生の指定(`Rgb`/`Theme{index,tint}`/`Indexed`)のまま保持する——実際の表示色への解決は別の表示用途の関心事(Issue #76)であり、本ファイルのdiff指向のスコープには不要。フォント色・罫線の線種/太さ/色(有無自体はIssue #97で対応済み)・斜体・下線などその他の `CT_Font`/`CT_Border` プロパティ、`wrapText`/`horizontal` 以外の `CT_CellAlignment` の属性(垂直方向配置、インデント、テキスト回転等)、`<diagonal>`罫線は、具体的な下流ユースケースが現れるまでスコープ外のまま(`Font` が既に採用している「完全な転写はしない」方針と同じ)。
2. ~~日付/時刻書式の判定ロジックの置き場所~~ → **解決**: [`parse/styles.rs`](../parse/styles.md) が `numFmtId`/`formatCode` から `ResolvedStyle::is_date_time` を判定するロジックを持つ（[resolve/style.md オープンクエスチョン2](../resolve/style.md) と同一の論点）。判定ヒューリスティックの精度自体は [parse/styles.md オープンクエスチョン2](../parse/styles.md) として引き続き未解決。
