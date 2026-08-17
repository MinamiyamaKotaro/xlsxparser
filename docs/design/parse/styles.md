# `parse/styles.rs` 設計書

*[English](styles.en.md)*

`src/parse/styles.rs` に対応する設計書。[architecture.md](../architecture.md) が定義する `parse/` の責務のうち「`styles.xml` のパース（fonts/fills/borders/numFmts/cellXfs）」を担う。`xl/styles.xml` をパースし、[`model/style.rs`](../model/style.md) が定義する `StyleSheet`（`cellXfs` インデックスから `ResolvedStyle` を引くテーブル）を構築する。[model/style.md オープンクエスチョン2](../model/style.md) と [resolve/style.md オープンクエスチョン2](../resolve/style.md) が共通して残していた「日付/時刻書式の判定ロジックの置き場所」を本ファイルの責務として確定させる。

## 責務・スコープ

- `<numFmts>`（カスタム数値書式定義）・`<cellXfs>`（実際にセルへ適用される書式定義の配列。インデックスが [`model::style::StyleId`](../model/style.md) に一致する）をパースする
- `<cellXfs>` の各 `<xf>` が参照する `numFmtId` を、組み込み書式ID（0〜163の範囲で固定的に意味が定義されているもの）または `<numFmts>` で定義されたカスタム書式のいずれかとして解決し、その書式が日付/時刻を表すか（`ResolvedStyle::is_date_time`）を判定する
- `<cellXfs>` のインデックス順に `StyleId` を割り当て、[`model::style::StyleSheet`](../model/style.md)（`HashMap<StyleId, Arc<ResolvedStyle>>`）を構築する
- **含まない責務**: `ResolvedStyle` をセルへ適用する処理そのもの（[`resolve/style.rs`](../resolve/style.md)）、`ResolvedStyle` / `StyleSheet` / `StyleId` の型定義そのもの（[`model/style.rs`](../model/style.md)）、フォント・塗りつぶし・罫線などの視覚的なスタイル要素の抽出（[model/style.md オープンクエスチョン1](../model/style.md) が未解決としているとおり、`ResolvedStyle` が現状 `is_date_time` のみを持つため、本ファイルもこれらのXML要素自体は読み飛ばす）

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
    // 1. <numFmts>を先に読み、numFmtId -> formatCode のマップを構築する
    //    （<numFmts>は<cellXfs>より前に出現するのがOOXMLの通例だが、
    //    仕様上の出現順保証はないため、必要なら2パス読み取りとする。
    //    オープンクエスチョン4参照）。
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
- `<numFmts>` が `<cellXfs>` より後に出現する `styles.xml`（XML仕様上は許容される順序）でも正しく解決できることの確認（オープンクエスチョン4の実装方針に対する回帰テスト観点）

## 未決事項 / オープンクエスチョン

1. **ロケール依存・和暦を含む日付書式（`numFmtId` 27〜36等）への対応要否**: 要求仕様書が「日本の業務システム」を主眼としているため、和暦（令和等）を含むカスタム日付書式への対応要否は要求仕様書の詳細化と合わせて確定させる。
2. **カスタム `formatCode` の日付/時刻判定ヒューリスティックの精度**: 条件付き書式の角括弧区間や、引用符・`\` でエスケープされたリテラル文字を正しく除外できるかは実装時の詳細設計に委ねる。誤判定の実害はエラー処理方針で述べたとおり限定的だが、精度そのものの向上余地は残る。
3. **未定義の `numFmtId` 参照に対するフォールバックとエラー化のどちらが適切か**: 現状は不正確ながら壊れた `styles.xml` をできるだけ読み進める方針（グレースフルデグラデーション）を仮定しているが、[resolve/style.md](../resolve/style.md) の `Error::InvalidStyleId`（セル側の `cellXfs` インデックス自体が不正な場合）との一貫性を取り、`styles.xml` 内部の参照不整合そのものも明確なエラーとして拒否すべきという意見もありうる。
4. **`<numFmts>` と `<cellXfs>` の読み取り順序**: OOXML上 `<numFmts>` は `<cellXfs>` より前に出現するのが通例だが、スキーマがこの順序を厳密に強制しているかは未確認。ストリーミングパースの1パスで完結させるか（`<numFmts>` が後から出現した場合に備え `<cellXfs>` 側の解決を一時的に保留する必要が生じる）、単純に2パス読み取りにするかは実装時に確定させる。
5. **フォント/塗りつぶし/罫線などの具体的なスタイル要素**: [model/style.md オープンクエスチョン1](../model/style.md) と同一の論点（未解決）。要求仕様書がセルスタイルとしてどこまでの要素をJSON出力に含める必要があるかは `json.rs` の設計、または要求仕様書自体の詳細化と合わせて確定させる。
6. **`applyNumberFormat` 属性・`cellStyleXfs`（名前付きセルスタイルの継承）への対応**: 現状は `<xf>` の `applyNumberFormat` 属性の値に関わらず `numFmtId` を直接権威あるものとして扱い、`xfId` が指す `cellStyleXfs` からの継承チェーンは考慮しない簡略化を仮定している。要求仕様書がスコープとする範囲でこの簡略化が十分かは未確定。
