# `parse/styles.rs` 設計書

*[English](styles.en.md)*

`src/parse/styles.rs` に対応する設計書。[architecture.md](../architecture.md) が定義する `parse/` の責務のうち「`styles.xml` のパース（fonts/fills/borders/numFmts/cellXfs）」を担う。`xl/styles.xml` をパースし、[`model/style.rs`](../model/style.md) が定義する `StyleSheet`（`cellXfs` インデックスから `ResolvedStyle` を引くテーブル）を構築する。[model/style.md オープンクエスチョン2](../model/style.md) と [resolve/style.md オープンクエスチョン2](../resolve/style.md) が共通して残していた「日付/時刻書式の判定ロジックの置き場所」を本ファイルの責務として確定させる。

## 責務・スコープ

- `<numFmts>`（カスタム数値書式定義）・`<cellXfs>`（実際にセルへ適用される書式定義の配列。インデックスが [`model::style::StyleId`](../model/style.md) に一致する）をパースする
- `<cellXfs>` の各 `<xf>` が参照する `numFmtId` を、組み込み書式ID（0〜163の範囲で固定的に意味が定義されているもの）または `<numFmts>` で定義されたカスタム書式のいずれかとして解決し、その書式が日付/時刻を表すか（`ResolvedStyle::is_date_time`）を判定する
- `<cellXfs>` のインデックス順に `StyleId` を割り当て、[`model::style::StyleSheet`](../model/style.md)（`HashMap<StyleId, Arc<ResolvedStyle>>`）を構築する
- `<fonts><font><sz val=".."/><b/>...</font>...</fonts>`(スキーマ上 `<numFmts>` と同様に `<cellXfs>` より前に出現)を位置インデックスの `Vec<Font>` としてパースし、各 `<xf>` の `fontId` 属性をその `Vec` に対して直接解決する——1 `<xf>` あたりO(1)、`<xf>` ごとに `<fonts>` を走査しない(Issue #38が要求するパフォーマンス要件)
- `<xf>` の子要素 `<alignment wrapText="1"/>`（Issue #37）を `ResolvedStyle::wrap_text` としてパースする——本ファイルが初めて扱う「`<xf>` の子要素」であり、`<xf>` の扱いを「開始タグ上で全て解決する」方式から、終了タグで確定させる `cur_xf` アキュムレータ方式へ再構成する必要があった（子要素を一切持たない、依然として一般的な自己終端 `<xf/>` 形式は即座に解決する）
- 各 `<xf>` の `numFmtId` を書式コード文字列 `ResolvedStyle::number_format` へ解決する(Issue #41)——組み込み(ECMA-376 Part 1 §18.8.30準拠のコンパイル時テーブル)・カスタム(`is_date_time_format` が既に行っている `<numFmts>` 引き当てと同じ)いずれも対象。ファイル内でユニークな `numFmtId` 単位でキャッシュし、文字列の確保が `StyleId`/セル数ではなく「ファイル内のユニークな `numFmtId` 数」に比例するようにする
- `<xf>` の子要素 `<alignment horizontal="..">`(Issue #42)を `ResolvedStyle::horizontal_alignment` としてパースする——`wrapText` が既に読んでいる同じ `<alignment>` 要素から読み取るため、追加の `<xf>` 走査は発生しない
- `<fills><fill><patternFill><fgColor .../><bgColor .../></patternFill></fill>...</fills>`(Issue #75、`<fonts>` と同様スキーマ上 `<cellXfs>` より前に出現)を位置インデックスの `Vec<Fill>` としてパースし、各 `<xf>` の `fillId` 属性をその `Vec` に対して直接解決する——`fontId` 解決と同じ「1 `<xf>` あたりO(1)」の形。生の `ColorRef` のまま保持し、`theme{N}.xml`/`tint` の解決は行わない(Issue #76の別の関心事)
- `<borders><border><left style=".."/><right .../><top .../><bottom .../><diagonal/></border>...</borders>`(Issue #97、`<fonts>`/`<fills>` と同様スキーマ上 `<cellXfs>` より前に出現)を位置インデックスの `Vec<model::style::Borders>` としてパースする——モデル型をそのまま収集要素として使う(`fillId` 方式ではなく `fontId` 方式: `<border>` エントリは `ResolvedStyle::borders` になるまでにさらなる変換を必要としない。パース済みの `<fill>` の `fgColor`/`bgColor` ペアとは異なる)。各辺の有無は `<left>`/`<right>`/`<top>`/`<bottom>` 自身の `style` 属性を直接読んで判定する(属性が存在し `"none"` でなければ `true`)——`<font>` に対して `<b>`/`<sz>` が既に使っている「入れ子を経由せず直近の子要素自身の開始/自己終端タグから属性を読む」のと同じ形。`<diagonal>` および各辺要素内にネストされる `<color>` は一切マッチさせず、ループの既定の無処理アームへ落ちる。各 `<xf>` の `borderId` 属性をその `Vec` に対して直接解決する——`fontId`/`fillId` 解決と同じ「1 `<xf>` あたりO(1)」の形
- **含まない責務**: `ResolvedStyle` をセルへ適用する処理そのもの（[`resolve/style.rs`](../resolve/style.md)）、`ResolvedStyle` / `StyleSheet` / `StyleId` / `Font` / `Alignment` / `ColorRef` / `Borders` の型定義そのもの（[`model/style.rs`](../model/style.md)）、`is_date_time`/`font`/`wrap_text`/`number_format`/`horizontal_alignment`/`fill_fg_color`/`fill_bg_color`/`borders` 以外の視覚的なスタイル要素の抽出(罫線の線種/太さ/色・`<diagonal>`・垂直方向配置・その他の配置属性。[model/style.md オープンクエスチョン1](../model/style.md) で未解決のまま)、`ColorRef::Theme`/`ColorRef::Indexed` を実際の表示RGB値へ解決すること(Issue #76、diff指向の本パーサには不要な表示用途の関心事)、`applyFont`/`applyAlignment`/`applyNumberFormat`/`applyBorder`/`cellStyleXfs` に基づく名前付きスタイル継承の解決(下記オープンクエスチョン6参照——`fontId`/`wrapText`/`numFmtId`/`horizontal`/`fillId`/`borderId` を無条件に直接使用する)

## 主要な型・関数（案）

```rust
use crate::error::Error;
use crate::model::style::{ResolvedStyle, StyleId, StyleSheet};
use crate::parse::{convert_xml_error, create_secure_reader, required_attr};
use std::collections::HashMap;
use std::io::BufRead;
use std::sync::Arc;

/// 組み込みnumFmtId（ECMA-376 Part 1 §18.8.30）のうち日付/時刻を表すID群。
/// 14〜22: 日付/時刻の組み込み書式（例: 14 = "mm-dd-yy"）。45〜47: 経過時間
/// （例: 46 = "[h]:mm:ss"）。27〜36系のロケール依存・和暦を含む日付書式は
/// 対応しない（オープンクエスチョン1参照）。
const BUILTIN_DATE_TIME_NUMFMT_IDS: &[u32] = &[14, 15, 16, 17, 18, 19, 20, 21, 22, 45, 46, 47];

/// `xl/styles.xml` をパースし、`StyleSheet` を構築する。
pub(crate) fn parse_styles(reader: impl BufRead, path: &str) -> Result<StyleSheet, Error> {
    let mut xml_reader = create_secure_reader(reader);
    // 実装方針:
    // 1. <numFmts>を先に読み、numFmtId -> formatCode のマップを構築する。
    //    ECMA-376 Part 1（SpreadsheetML）§18.8.39 CT_Stylesheetの
    //    xsd:sequenceがnumFmts, fonts, fills, borders, cellStyleXfs,
    //    cellXfs, ... の出現順を仕様として強制するため、単純な1パスの
    //    ストリーミングパースで足りる（オープンクエスチョン4を解決）。
    // 2. <cellXfs>の各<xf>についてnumFmtId属性（省略時は既定値0=General）を
    //    読み、is_date_time_formatで判定し、ResolvedStyleを構築する。
    // 3. <cellXfs>内のインデックス（0始まり）をStyleIdとしてStyleSheetへ格納する。
    let mut num_fmts: HashMap<u32, String> = HashMap::new();
    let mut stylesheet: StyleSheet = HashMap::new();
    let _ = (&mut xml_reader, path, &mut num_fmts, &mut stylesheet);
    unimplemented!()
}

/// `numfmt_id` と、カスタム書式の場合の `format_code`（`num_fmts` からの
/// 引き当て結果。見つからない場合は `None`）から、その書式が日付/時刻を
/// 表すかを判定する。
///
/// - `numfmt_id < 164`（組み込み）: `BUILTIN_DATE_TIME_NUMFMT_IDS` に含まれるかで判定。
/// - `numfmt_id >= 164`（カスタム）: `format_code` をヒューリスティックに走査し、
///   日付/時刻を表すトークン（`y`, `m`, `d`, `h`, `s` 等。`\` エスケープや
///   引用符で囲まれたリテラル、`[Red]` のような条件付き書式の角括弧区間は
///   除外する）の有無で判定する。この判定は完全ではない（オープンクエスチョン2参照）。
/// - `numfmt_id` が組み込みでもカスタム定義にも見つからない場合、
///   `is_date_time: false` へフォールバックする（エラーにしない。
///   エラー処理方針参照）。
fn is_date_time_format(numfmt_id: u32, format_code: Option<&str>) -> bool {
    let _ = (numfmt_id, format_code);
    unimplemented!()
}
```

**フォント解決(Issue #38、実装後に追記)**: `<fonts>` は `<numFmts>` と同様の方法でストリームし、出現順の `Vec<Font>`(インデックス = `fontId`)へ収集する。その上で各 `<cellXfs><xf>` の `fontId` 属性(欠落時は`0`、パース不能または `Vec` の範囲外の場合は `Font::default()` ——`numFmtId` が既に採用している段階的縮退方針と同じ)で `Vec::get` により直接引き当てる。`<font>` 自身の `<sz val="..">` が `size_pt` を、`<b/>`(`val`なし)または `<b val="1"/>`/`<b val="true"/>` が `bold: true` を設定し、比較的稀な明示的な `<b val="0"/>`/`<b val="false"/>` は `bold: false` を設定する。`applyFont` は一切参照せず、`<cellStyleXfs>`/`xfId` に基づく名前付きスタイル継承も解決しない——オープンクエスチョン6参照。

**折返し解決(Issue #37、実装後に追記)**: `numFmtId`/`fontId`(いずれも `<xf>` 自身の開始タグ上の単純な属性)と異なり、`wrapText` は*子要素*である `<xf><alignment wrapText="1"/></xf>` 上に存在する。`<alignment>` 子要素を持つ `<xf>` はもはや単純な自己終端タグにはなり得ないため、`<xf>` の扱いを `CurXf { numfmt_id, font_id, wrap_text }` アキュムレータ(`<font>` に対してすでに確立されている `cur_font` と同じ、開始タグ〜終了タグ間の `Option<T>` パターン)を中心に再構成した: `Start` タグで `numFmtId`/`fontId` を即座に読み `cur_xf` を開き、ネストした `<alignment>`(あれば)が `cur_xf.wrap_text` を更新し、`ResolvedStyle` 自体は対応する `End` タグでのみ構築・格納する。子要素を一切持たない、依然として一般的な `<xf/>` の場合は独立した `Empty` 分岐として扱い、`cur_xf` に一切触れずに即座に解決・格納する——両方の分岐は同じ `push_resolved_style` ヘルパーへ収束するため、`ResolvedStyle` を構築する箇所は一箇所のみとなる。`wrapText` 自体は属性(または `<alignment>` 要素全体)が欠落している場合 `false` がデフォルトであり、`"1"`/`"true"` のみが `true` として扱われる——`xsd:boolean` の真値表現に合わせたもの。`applyAlignment` は一切参照しない、`applyFont` に対する方針と同じ。

**書式コード解決(Issue #41、実装後に追記)**: `numFmtId` の値域はきれいに二分できる——ECMA-376上、ID 0〜163は仕様が固定的に意味を定義する組み込みID、カスタム `<numFmts>` エントリは仕様上決してID 164未満には割り当てられないため、`if numfmt_id < 164` の単純な分岐のみで済み、2つの情報源の間で優先順位を考える必要は一切ない(`is_date_time_format` が既に前提としている分割と同じ)。組み込みテーブル(`BUILTIN_NUMFMT_CODES`——ECMA-376 §18.8.30が実際に定義する書式コードを収めたコンパイル時 `&[(u32, &str)]`。1〜22, 37〜49が対象で、23〜36・50〜163は仕様自体が具体的な書式コードを定めていないアプリケーション/国際化用の予約領域のため対象外)は、既存の `BUILTIN_DATE_TIME_NUMFMT_IDS`(メンバーシップのみの一覧)とは別テーブルとして新設する——両者は用途が異なる(日付判定はメンバーシップのみで足りるが、表示用には実際の文字列そのものが必要)ため、統合せず独立させた。`numFmtId=0`(「General」)・`numFmtId` 属性の欠落・いずれの表にも見つからないIDは、いずれも `None`(「報告すべき情報なし」)へ解決される——`is_date_time_format` が未解決IDに対して既に採用しているグレースフルデグラデーション方針と同じ(`None` であって `Some("General")` ではない理由は `model/style.md` の `ResolvedStyle::number_format` ドキュメントコメント参照)。`resolved_formats: HashMap<u32, Arc<str>>` キャッシュ(`num_fmts` と並行し、遅延的に構築)が `numFmtId` ごとの解決結果をメモ化するため、書式コード文字列はファイル内でその `numFmtId` を参照する `<xf>` エントリ数(ひいてはセル数)に関わらず、ユニークな `numFmtId` ごとに高々1回しか確保されない——Issue #41が明示的に要求するパフォーマンス要件そのもの。解決処理自体が `<xf>` ごと(＝`StyleId` ごと)に1回だけ走り、セルごとには走らないこと、および `resolve/style.rs` がセルごとに複製するのは外側の `Arc<ResolvedStyle>` 1個のみで内側の `Option<Arc<str>>` を個別に触ることは無いことから、本フィールドによるセル単位の追加コストはシートの規模に関わらず実質ゼロである——README.md記載の `extreme_sparse.xlsx` ベンチマークを再計測し(3.2ms、既存記載の3.0msから誤差の範囲内)、30万セル規模の高密度スタイル付きシート(約299ms、Issue #40のベンチマーク時点の基準値と整合)でも確認済み。

**水平方向配置解決(Issue #42、実装後に追記)**: `<xf><alignment horizontal="..">` は `wrapText`(Issue #37)が既に読んでいる `cur_xf.is_some()` の同じ分岐内、同じ `<alignment>` 要素から `horizontal` を読み取るため、独立した分岐や追加の `<xf>` 走査は一切発生しない——Issue #42が明示的に要求するパフォーマンス要件を直接満たす。`horizontal` の値域はECMA-376 `ST_HorizontalAlignmentValues`(`general`/`left`/`center`/`right`/`fill`/`justify`/`centerContinuous`/`distributed`)であり、`"general"`・属性欠落(`<alignment>` 要素自体が存在しない自己終端 `<xf/>` の場合を含む)・未知の値はいずれも `Alignment::General` へフォールバックする——`numFmtId`/`fontId` が既に採用しているのと同じグレースフルデグラデーションの形。`Alignment` は小さな `Copy` な列挙型(`model/style.md` 参照)であるため、`CurXf`・`push_resolved_style` を経由して受け渡すコストは `wrap_text: bool` が既に払っているコストと変わらない。

**塗りつぶし色解決(Issue #75、実装後に追記)**: `<fills>` は `<fonts>` と同じ方法でストリームし、出現順の `Vec<Fill>`(本ファイル内のみのプライベート構造体、`{fg_color, bg_color}`——本モジュール外には公開しない)へ収集する。その上で各 `<cellXfs><xf>` の `fillId` 属性で `Vec::get` により直接引き当てる——`fontId` 解決と全く同じ形(`resolved_formats` のような別途メモ化キャッシュは不要: `ColorRef::Rgb` の `Arc<str>` により、引き当てた `Fill` の `.cloned()` は既に参照カウント操作であり新規確保にならないため——`model/style.md` 参照)。`<fill>` の子要素 `<patternFill>` は独自のネストした読み取りループ(`parse_fill_body`。`parse/drawing.rs::parse_marker` の「自前のローカルバッファで自分自身の終了タグまで読む」形を踏襲)でパースし、`<fgColor>`/`<bgColor>` を読み取る——各要素は `parse_color` で解決し、`rgb`(そのまま `Arc<str>`)→`theme`+任意の`tint`→`indexed` の順に確認する(ECMA-376 `CT_Color` の3表現は互いに排他的)。`theme`/`indexed` の数値がパース不能な場合、および色要素が3属性のいずれも持たない場合(例: `auto="1"`)は、いずれも `None` へ縮退する——`numFmtId`/`fontId`/`fillId` が既に採用しているグレースフルデグラデーション方針と同じであり、エラーにはしない。`theme{N}.xml` は一切読まず、`tint` の適用も行わない——Issue #76の別の表示用途の関心事。

## 依存関係

- 依存先: [`parse/mod.rs`](mod.md)（`create_secure_reader`, `convert_xml_error`, `required_attr`）、[`model/style.rs`](../model/style.md)（`ResolvedStyle`, `StyleId`, `StyleSheet`）、[`error.rs`](../error.md)
- 依存元: [`resolve/style.rs`](../resolve/style.md)（構築済みの `StyleSheet` を引いてセルへ適用する）、`pipeline.rs`（フェーズ1〜3の間で一度だけ構築し、`resolve_sheet` の呼び出しに渡す。architecture.md 「フェーズ4完了時に `StyleSheet` を破棄する」に従い、全シートの解決が終わった時点で破棄する）

[model/style.md 依存関係](../model/style.md) が「`resolve/` と `parse/` の双方が `model/style.rs` にのみ依存し、互いには直接依存しない構造とする」としていた設計をそのまま実装する。本ファイル（構築主体）と `resolve/style.rs`（適用主体）は互いを知らず、`StyleSheet` という共有語彙のみを介して間接的につながる。

## エラー処理方針

- `<numFmts>` / `<cellXfs>` の構造自体が破損している（XML構文エラー）場合は [`convert_xml_error`](mod.md) を通じて `Error::XmlParse` または `Error::ZipBombDetected` に変換する
- `<xf>` の `numFmtId` 属性が省略された場合は既定値 `0`（`"General"`、非日付）として扱う（`Error::MissingRequiredElement` にしない。OOXML上 `numFmtId` はオプション属性であるため）
- **`numFmtId` が組み込みIDにもカスタム `<numFmts>` の定義にも見つからない場合はエラーにせず `is_date_time: false` へフォールバックする**。これは [resolve/style.md エラー処理方針](../resolve/style.md) が採用する「個々の値解釈の緩やかな失敗はドキュメント全体の整合性を損なわない限りエラーにしない」という方針の延長であり、破損・非標準な `styles.xml` に対してもできる限り読み進めるグレースフルデグラデーションを優先する（オープンクエスチョン3で別案も検討する）
- カスタム書式のヒューリスティックな日付/時刻判定が誤った場合（偽陽性・偽陰性）の実害は限定的である: 偽陽性（非日付をDateTimeへ変換しようとする）は [resolve/style.md](../resolve/style.md) の `serial_to_date_time` が変換不能値に対して行うフォールバック（`CellValue::Number` を維持）でさらに緩和され、偽陰性（日付をNumberのまま維持）はセルの値自体は失われない

## テスト方針

- 組み込み `numFmtId`（例: `14` = `"mm-dd-yy"`）を参照する `<xf>` が `is_date_time: true` として解決されることの確認
- 組み込み `numFmtId` のうち日付/時刻に該当しないID（例: `0` = `"General"`, `9` = `"0%"`）が `is_date_time: false` となることの確認
- カスタム書式（`numFmtId >= 164`）で `<numFmts>` に `formatCode="yyyy/mm/dd"` のような定義がある場合に `is_date_time: true` と判定されることの確認
- カスタム書式で `formatCode` が日付/時刻を含まない書式（例: `"#,##0.00"`, `"@"`）の場合に `is_date_time: false` と判定されることの確認
- カスタム書式で `formatCode` が条件付き書式やエスケープ文字を含む場合（例: `"[Red]#,##0;[Blue]-#,##0"`）に、日付関連トークンの誤検出により `is_date_time: true` へ誤判定されないことの確認（ヒューリスティックの精度に関する回帰テスト観点）
- `numFmtId` が組み込みにもカスタム定義にも見つからない場合に、エラーを返さず `is_date_time: false` へフォールバックすることの確認
- `numFmtId` 属性が省略された `<xf>` がデフォルト値 `0`（`General`、非日付）として扱われることの確認
- `<cellXfs>` 内の複数 `<xf>` から構築した `StyleSheet` のキー（`StyleId`）が `<cellXfs>` の0始まりインデックス順と一致することの確認（[resolve/style.md](../resolve/style.md) との結線）
- 仕様上正当な順序（`<numFmts>` が `<cellXfs>` より前）の `styles.xml` が1パスで正しく解決できることの確認。逆順（`<numFmts>` が `<cellXfs>` より後）はECMA-376のスキーマ違反となる非準拠ファイルであり本設計の対象外だが、そのような入力が渡された場合でも `panic` せず、該当 `numFmtId` が「見つからない」ケースとして `is_date_time: false` へフォールバックし処理を継続できることの確認（オープンクエスチョン4の解決に対する回帰テスト観点）
- **異なる `fontId` を持つ複数の `<xf>` が、対応する `<fonts>` エントリの正しい `size_pt`/`bold` に解決されることの確認**(Issue #38)
- **`<b val="0"/>`/`<b val="false"/>` が `bold: false` に解決されることの確認**(要素が単に存在しない場合とは異なる、明示的な「太字でない」形式)
- **`fontId` 属性を持たない `<xf>` が `<fonts>` の先頭エントリ(インデックス0)にデフォルトすること**、未定義の `<font>` を参照する範囲外の `fontId` が `Font::default()` にフォールバックすること、`<fonts>` 要素自体が無いファイルでは全スタイルが `Font::default()` になること、子プロパティを持たない空の `<font/>` は `Font::default()` として登録されること——いずれもエラーにせず段階的に縮退することの確認
- **`<xf><alignment wrapText="1"/></xf>` が `wrap_text: true` に解決されることの確認**、`wrapText` 属性を持たない `<alignment>`(例: `horizontal="center"` のみ)や `wrapText="0"` の場合は `false` に解決されること、`<alignment>` 子要素を一切持たない自己終端 `<xf/>` も `false` に解決されることの確認(Issue #37)
- **`<alignment>` 子要素を持つ `<xf>` でも `numFmtId`/`fontId` が正しく解決されることの確認**——子要素対応のために行った `Start`/`End` への再構成が、`<xf>` の開始タグから直接読み取る属性を壊していないことの回帰テスト
- **自己終端の `<xf/>` エントリと `<alignment>` 子要素を持つ `<xf>...</xf>` エントリが混在する `<cellXfs>` でも、文書順に `StyleId` が割り当てられることの確認**——両方の分岐が同じ挿入ポイントへ収束する必要がある
- **組み込み `numFmtId`(例: `9`)が既知のコード(`"0%"`)へ解決されることの確認**(Issue #41)
- **カスタム `numFmtId`(`164`以上、`<numFmts>` に定義あり)がその `formatCode` 文字列へ解決されることの確認**
- **`numFmtId=0`・`numFmtId` 属性の欠落・組み込みテーブルにも `<numFmts>` にも見つからないIDが、いずれも `number_format: None` へ解決されることの確認**(「Generalは報告すべき情報なし」という方針の境界値・回帰テスト)
- **日付/時刻判定済み(`is_date_time: true`)の `<xf>` でも `number_format` が独立して保持されることの確認**——2つのフィールドは独立に解決され、一方が他方を暗黙的にクリアしない
- **同一の `numFmtId` を参照する複数の `<xf>` が、`Arc::ptr_eq` で同一と判定される `number_format` へ解決されることの確認**——`resolved_formats` メモ化キャッシュの回帰テスト
- **`<alignment horizontal="left"/>`/`"center"`/`"right"`/`"fill"`/`"justify"`/`"centerContinuous"`/`"distributed"` それぞれが対応する `Alignment` バリアントへ解決されることの確認**(Issue #42)
- **`horizontal="general"`・`horizontal` 属性の欠落・未知の値がいずれも `Alignment::General` へ解決されること**、および `<alignment>` 子要素を一切持たない自己終端 `<xf/>` も `Alignment::General` へ解決されることの確認(境界値・グレースフルデグラデーション)
- **`<alignment wrapText="1" horizontal="center"/>` が `wrap_text: true` と `horizontal_alignment: Alignment::Center` の両方を同じ要素から正しく解決することの確認**——両属性を1つのイベントから読み取る実装が互いを壊していないことの回帰テスト
- **`<fgColor rgb="..">`/`theme=".." tint="..">`/`indexed="..">` それぞれが対応する `ColorRef` バリアントへ解決されることの確認**——theme指定でtintが無い場合(`tint: None`、明示的な`Some(0.0)`とは区別)を含む(Issue #75)
- **`<fgColor>`/`<bgColor>` を一切持たない `<fill>`(`patternType="none"`/`"gray125"`)が両方とも `None` へ解決されることの確認**、および `<fgColor auto="1"/>`(`rgb`/`theme`/`indexed`のいずれも持たない)も `None` へ解決されることの確認
- **パース不能な `theme`/`indexed` の数値がエラーにせず `None` へ縮退することの確認**(`numFmtId`/`fontId`/`fillId` と同じグレースフルデグラデーション方針)、および `<fill>` の終了タグ前でEOFに達した場合に `Error::MissingRequiredElement` として報告されることの確認
- **`fillId` 属性を持たない `<xf>` が `<fills>` の先頭エントリにデフォルトすること**、および同一の `fillId` を参照する複数の `<xf>` が `Arc::ptr_eq` で同一と判定される `ColorRef::Rgb` へ解決されることの確認——共有`fillId`のクローンが新規確保ではなく参照カウント操作であることの回帰テスト(`model/style.md` 参照)
- **`<left>`/`<right>`/`<top>`/`<bottom>` の全てに`style`属性が存在(かつ`"none"`でない)する`<border>`が`Borders { top: true, right: true, bottom: true, left: true }`へ解決されることの確認**(Issue #97)
- **辺要素が完全に欠落・`style`属性なしで自己終端・`style="none"`のいずれの場合もその辺が`false`へ解決されることの確認**、および完全に空の`<border/>`/`<border></border>`が`Borders::default()`(全て`false`)へ解決されることの確認——`fillId`の「報告すべき情報なし」方針と同じグレースフルデグラデーション/境界値ケース
- **一部の辺のみ`style`を持つ場合に混在した`Borders`値が正しく解決されることの確認**(例: 上下のみ、よくある「行区切り線」の形)
- **`<diagonal style="..">`が兄弟要素であっても`top`/`right`/`bottom`/`left`のいずれにも影響しないことの確認**、および辺要素内にネストされた`<color>`(例: `<left style="thin"><color indexed="64"/></left>`)がその辺自身の`style`属性の読み取りを妨げないことの確認
- **`borderId`属性を持たない`<xf>`が`<borders>`の先頭エントリにデフォルトすること**、および範囲外の`borderId`が`Borders::default()`へフォールバックすることの確認——`fontId`/`fillId`と同じグレースフルデグラデーションの形
- **同一`<xf>`上で`ResolvedStyle::borders`が`fill_fg_color`/`fill_bg_color`/`font`/`wrap_text`と独立に解決されることの確認**——4つの`<xf>`子要素/属性の解決が互いに干渉しないことの回帰テスト

## 未決事項 / オープンクエスチョン

1. **ロケール依存・和暦を含む日付書式（`numFmtId` 27〜36等）への対応要否**: 要求仕様書が「日本の業務システム」を主眼としているため、和暦（令和等）を含むカスタム日付書式への対応要否は要求仕様書の詳細化と合わせて確定させる。
2. **カスタム `formatCode` の日付/時刻判定ヒューリスティックの精度**: 条件付き書式の角括弧区間や、引用符・`\` でエスケープされたリテラル文字を正しく除外できるかは実装時の詳細設計に委ねる。誤判定の実害はエラー処理方針で述べたとおり限定的だが、精度そのものの向上余地は残る。
3. **未定義の `numFmtId` 参照に対するフォールバックとエラー化のどちらが適切か**: 現状は不正確ながら壊れた `styles.xml` をできるだけ読み進める方針（グレースフルデグラデーション）を仮定しているが、[resolve/style.md](../resolve/style.md) の `Error::InvalidStyleId`（セル側の `cellXfs` インデックス自体が不正な場合）との一貫性を取り、`styles.xml` 内部の参照不整合そのものも明確なエラーとして拒否すべきという意見もありうる。
4. ~~`<numFmts>` と `<cellXfs>` の読み取り順序~~ → **解決**: 単純な1パスのストリーミングパースで実装する（[PR #9 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204)を反映）。ECMA-376 Part 1（SpreadsheetML）§18.8.39 `CT_Stylesheet` の `xsd:sequence` が `numFmts`, `fonts`, `fills`, `borders`, `cellStyleXfs`, `cellXfs`, `cellStyles`, `dxfs`, `tableStyles`, `colors`, `extLst` の出現順を仕様として強制しており、`numFmts` が `cellXfs` より後に出現するファイルは仕様上無効なOOXMLドキュメントとなるため、2パス読み取りは不要と判断する。なお、この順序に従わない非準拠なファイルに実際に遭遇した場合でも、該当 `numFmtId` は「組み込みにもカスタム定義にも見つからない」ケースとしてエラー処理方針が定める `is_date_time: false` へのフォールバックが働くため、クラッシュせず安全側に縮退する。
5. **フォント/塗りつぶし/罫線などの具体的なスタイル要素**: さらに解決が進んだ——`font: Font { size_pt, bold }`(Issue #38)、`wrap_text: bool`(Issue #37)、`number_format: Option<Arc<str>>`(Issue #41)、`horizontal_alignment: Alignment`(Issue #42)、`fill_fg_color`/`fill_bg_color: Option<ColorRef>`(Issue #75)、`borders: Borders`(Issue #97、辺ごとの有無のみ、`<diagonal>`は対象外)をいずれも上記の通り実装済み。[Issue #36](https://github.com/MinamiyamaKotaro/xlsxparser/issues/36) 配下のサブIssueに加え、派生の塗りつぶし色・罫線Issueも解決済み。罫線の線種/太さ/色・垂直方向配置は、具体的な下流ユースケースが現れるまでスコープ外のまま。`ColorRef` を実際の表示RGB値へ解決すること(`theme{N}.xml`/`tint`/レガシーindexedパレット)はIssue #76として別途追跡している——diff指向の本パーサには不要な表示用途の関心事。
6. **`applyNumberFormat`/`applyFont`/`applyAlignment`/`applyBorder` 属性・`cellStyleXfs`（名前付きセルスタイルの継承）への対応**: 解決——意図的な簡略化として、Issue #38(numFmt→フォント)、Issue #37(→折返し)、Issue #41(書式コード解決自体——`is_date_time` が元々無条件に読んでいた同じ `numFmtId` を再利用)、Issue #42(→水平方向配置)、Issue #97(→罫線有無)と同じ方針を拡張してきた。`<xf>` の `applyNumberFormat`/`applyFont`/`applyAlignment`/`applyBorder` 属性の値に関わらず `numFmtId`/`fontId`/`borderId`/`<alignment wrapText>`/`<alignment horizontal>` を直接権威あるものとして扱い、`xfId` が指す `cellStyleXfs` からの継承チェーンは考慮しない。実務上、`<cellXfs>` 内の `<xf>` は `apply*` フラグの値に関わらず、実際に使用する書式を直接持つ(これらの属性は「読み込み時にその属性が適用されるか」ではなく「ユーザーがその属性を明示的にカスタマイズしたか」を示すUI上のヒントに近い)。`cellStyleXfs` の完全な継承(「標準」「見出し1」等の名前付きセルスタイルの解決)は、まだどの下流ユースケースからも要求されていない、質的により大きな機能である。この簡略化が誤った `font`/`wrap_text`/`number_format`/`horizontal_alignment`/`is_date_time` を生む具体的なケースが見つかった場合は再検討する。
