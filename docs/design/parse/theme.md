# `parse/theme.rs` 設計書

*[English](theme.en.md)*

`src/parse/theme.rs` に対応する設計書。[architecture.md](../architecture.md) が定義する `parse/` の責務のうち「`theme{N}.xml` のパース」（[Issue #76](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76)）を担う。`xl/theme/theme{N}.xml` の `<a:clrScheme>` をパースし、[`model/color.rs`](../model/color.md) が定義する `ThemePalette` を構築する。

## 責務・スコープ

- `<a:clrScheme>` 直下の12要素(`dk1`/`lt1`/`dk2`/`lt2`/`accent1`〜`accent6`/`hlink`/`folHlink`)のみをストリーム走査し、それぞれの子要素 `<a:srgbClr val="RRGGBB"/>` または `<a:sysClr val="..." lastClr="RRGGBB"/>` から実RGB値を読み取る。図形スタイル・フォントスキームなど `<clrScheme>` 以外の要素は一切解釈しない([Issue #76 設計提案](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)がそもそも要求する範囲)
- 読み取った12色を、[`model::color::ThemePalette`](../model/color.md) が契約するインデックス順(`0:lt1, 1:dk1, 2:lt2, 3:dk2, 4..=9:accent1..=6, 10:hlink, 11:folHlink`——XML宣言順`dk1,lt1,...`からスロット0/1が入れ替わる)へ配置して返す。この入れ替えはPoC検証で実データ・Apache POIの `ThemesTable.ThemeElement` enum 双方に対して確認済み([Issue #76コメント](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352366260))
- `<a:sysClr val="windowText" lastClr="000000"/>` のような要素は `lastClr` 属性(Excelが保存時に書き込むキャッシュ値)を実RGB値として採用する。`lastClr` が欠落・不正な16進数の場合は、スロット名に応じたフォールバック値へ縮退する(`lt1`/`lt2` → 白、`dk1`/`dk2`/その他 → 黒。[Issue #76コメント](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352388486)の確定仕様)——PoCでリポジトリ同梱の全フィクスチャ(`<a:sysClr>` 84要素)を走査した結果、実世界のExcel生成ファイルでは `lastClr` が欠落することは無かった。このフォールバックは専ら細工・破損ファイルに対する防御的コードであり、通常経路では発火しない
- **含まない責務**: `Rgb`/`ThemePalette` の型定義そのもの([`model/color.rs`](../model/color.md))、`tint` 補正・レガシー64色インデックスパレットの解決([`resolve/color.rs`](../resolve/color.md)、未設計——`tint` は `theme{N}.xml` 自体には存在せず参照側の `styles.xml` に個別に付与されるため、そもそも本ファイルが扱う情報ではない)、`theme{N}.xml` パーツの実体パスの解決(`xl/_rels/workbook.xml.rels` からのリレーションシップ解決。`pipeline.rs`、[pipeline.md オープンクエスチョン6](../pipeline.md)参照——本関数は既にパスが解決済みの `reader` を受け取る前提とする、`parse/styles.rs`/`parse/shared_strings.rs` と同じ形)、`theme{N}.xml` パーツ自体を読み込むかどうかの判断(「pay-for-what-you-use」——`StyleSheet` が `ColorRef::Theme` を1件も含まない場合に本パースを完全にスキップする最適化は呼び出し元 `pipeline.rs` の責務。[pipeline.md オープンクエスチョン6](../pipeline.md)参照)

## 主要な型・関数（案）

```rust
use crate::error::Error;
use crate::model::color::{Rgb, ThemePalette};
use crate::parse::{convert_xml_error, create_secure_reader, optional_attr};
use quick_xml::events::Event;
use std::io::BufRead;

/// `<clrScheme>` が持つ12個の名前付きスロット。宣言順ではなく、
/// `ThemePalette` が契約する解決後インデックス順(0:lt1, 1:dk1, ...)で
/// 並べてある——本テーブル自体が「名前→出力インデックス」の対応表を兼ねる。
const SLOT_NAMES: [&str; 12] = [
    "lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4",
    "accent5", "accent6", "hlink", "folHlink",
];

/// `xl/theme/theme{N}.xml` をパースし、`ThemePalette` を構築する。
/// `path` は既に解決済みのパーツパス(呼び出し元の責務。含まない責務参照)。
pub(crate) fn parse_theme(reader: impl BufRead, path: &str) -> Result<ThemePalette, Error> {
    let mut xml_reader = create_secure_reader(reader);
    // 実装方針:
    // 1. <a:clrScheme>の直下に現れる12要素を、名前空間プレフィックスを
    //    無視した局所名(local_name)でSLOT_NAMESと突合しながらストリーム走査する。
    //    schema上<clrScheme>の子要素順序は宣言順(dk1,lt1,dk2,lt2,...)に
    //    固定されているが、本パーサーは名前で照合するため順序に依存しない
    //    (オープンクエスチョン1参照)。
    // 2. 各スロットの子要素<a:srgbClr val="RRGGBB"/>または
    //    <a:sysClr val="windowText" lastClr="RRGGBB"/>からRGB値を読み取り、
    //    resolve_slot_colorへ委譲する。
    // 3. 12スロットのいずれかが最後まで見つからなかった場合
    //    (<clrScheme>自体の欠落、または子要素の一部欠落)は
    //    Error::MissingRequiredElementを返す——numFmtIdの欠落等とは異なり、
    //    ThemePaletteは12要素すべてが揃って初めて意味を持つ固定長配列
    //    であるため、部分的な構築を許さない(エラー処理方針参照)。
    // 4. 12スロット全て解決できたら、SLOT_NAMESの並び順(=ThemePaletteの
    //    契約するインデックス順)そのままの[Rgb; 12]としてThemePaletteを返す。
    let _ = (&mut xml_reader, path);
    unimplemented!()
}

/// 1スロット分の色要素(`<a:srgbClr>`または`<a:sysClr>`)を実RGB値へ解決する。
/// `slot_name`はフォールバック値の決定にのみ使う(下記参照)。
///
/// - `<a:srgbClr val="RRGGBB"/>`: `val`をそのまま6桁16進数としてパースする。
/// - `<a:sysClr val="..." lastClr="RRGGBB"/>`: `val`(名前付きシステム色。
///   `windowText`/`window`等)はOS非依存に解決する手段がないため無視し、
///   `lastClr`(Excelが保存時に書き込んだキャッシュ値)を採用する
///   ([Issue #76コメント](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352388486)。
///   他実装(Apache POI含む)も採用する現実的な妥協点)。
/// - `lastClr`属性自体が欠落、または6桁16進数として不正な場合:
///   `slot_name`が`lt1`/`lt2`なら`#FFFFFF`、それ以外(`dk1`/`dk2`/`accent*`/
///   `hlink`/`folHlink`)なら`#000000`へフォールバックする——エラーにしない
///   (エラー処理方針参照)。
fn resolve_slot_color(slot_name: &str, event: &Event<'_>, path: &str) -> Result<Rgb, Error> {
    let _ = (slot_name, event, path);
    unimplemented!()
}
```

## 依存関係

- 依存先: [`parse/mod.rs`](mod.md)（`create_secure_reader`, `convert_xml_error`, `optional_attr`）、[`model/color.rs`](../model/color.md)（`Rgb`, `ThemePalette`）、[`error.rs`](../error.md)
- 依存元: `pipeline.rs`（`theme{N}.xml` パーツが存在し、かつ `StyleSheet` が `ColorRef::Theme` を含む場合にのみ呼び出す想定——[pipeline.md オープンクエスチョン6](../pipeline.md)参照）、[`resolve/color.rs`](../resolve/color.md)（構築済みの `ThemePalette` を読んで `ColorRef::Theme` を解決する。本ファイルには依存しない——[`model/style.md`](../model/style.md) の `parse/`・`resolve/` 分離方針と同じく、両者は `model/color.rs` の型のみを介して間接的につながる）

## エラー処理方針

- `<clrScheme>` の構造自体が破損している(XML構文エラー)場合は [`convert_xml_error`](mod.md) を通じて `Error::XmlParse` または `Error::ZipBombDetected` に変換する
- **12スロットのいずれかが最後まで見つからない場合は `Error::MissingRequiredElement` を返す**——`parse/styles.rs` が `numFmtId` の欠落・不整合に対して採用するグレースフルデグラデーション方針とは意図的に異なる。`numFmtId` は「見つからない」こと自体が仕様上あり得る正当な状態(`None` へ縮退すればよい)なのに対し、`ThemePalette` は12要素固定長配列であり、一部だけ構築された `ThemePalette` を返す設計は [`model/color.md`](../model/color.md) の型契約自体を壊す。`<clrScheme>` は ECMA-376 上必須の12要素を持つことが仕様で保証されているため、欠落は「読み込み時に許容すべき曖昧さ」ではなく「破損したファイル」として扱う
- **個々のスロットの色表現(`sysClr`の`lastClr`欠落・不正な16進数)はエラーにせず、スロット名に応じた固定フォールバック値へ縮退する**——`resolve_slot_color`のドキュメント参照。これは「要素自体は存在するが値の解釈があいまい」なケースであり、`numFmtId`が採用する個々の値解釈レベルのグレースフルデグラデーションと同じ位置づけ(要素の欠落そのものとは区別する)

## テスト方針

- 実フィクスチャ(`tests/fixtures/complex/styled_fill_color.xlsx`)の `theme1.xml` から、PoC検証済みの実際の値(`dk1=000000, lt1=FFFFFF, dk2=1F497D, lt2=EEECE1, accent1=4F81BD, accent2=C0504D, ..., hlink=0000FF, folHlink=800080`)が、スワップ済みインデックス順(`palette.0[0] == lt1の値`、`palette.0[1] == dk1の値`)で正しく `ThemePalette` へ格納されることの確認——[Issue #76コメント](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352366260)のPoCをユニットテストへ昇格させたもの
- `<clrScheme>` の子要素がXML宣言順(`dk1,lt1,dk2,lt2,...`)通りに現れる、仕様上正当な入力に対して正しく解決できることの確認
- **`<clrScheme>` の12要素のいずれか(例: `accent3`)が欠落している場合に `Error::MissingRequiredElement` を返すことの確認**(構造的な欠落に対するfail closed方針の回帰テスト)
- `<a:srgbClr val="4F81BD"/>` が `Rgb { r: 0x4F, g: 0x81, b: 0xBD }` へ正しく解決されることの確認
- `<a:sysClr val="windowText" lastClr="000000"/>` が `lastClr` の値(`#000000`)へ解決され、`val` の値(`windowText`)は無視されることの確認
- **`lastClr` 属性を持たない `<a:sysClr val="windowText"/>` が、`dk1`/`dk2`スロットでは `#000000` へ、`lt1`/`lt2`スロットでは `#FFFFFF` へフォールバックすることの確認**(実フィクスチャでは発火しない経路のため、合成XMLで明示的にテストする——[Issue #76コメント](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352422163)が指摘した、自然にはカバレッジが付かない分岐)
- `lastClr` が不正な16進数(例: `lastClr="ZZZZZZ"`)の場合も同じフォールバック値へ縮退し、パニックしないことの確認
- 名前空間プレフィックスが `a:` 以外(またはプレフィックスなし)で宣言された `<clrScheme>` でも局所名一致により正しく解決できることの確認(オープンクエスチョン1参照)

## 未決事項 / オープンクエスチョン

1. **名前空間プレフィックスの扱い**: [parse/mod.md オープンクエスチョン4](mod.md) が `r:id` 等について「`quick_xml::NsReader` によるURIベースの解決は採用せず、プレフィックス込みの文字列前方一致で簡略化する」と決めているのに対し、`<clrScheme>` 配下の要素は `local_name()`(プレフィックスを無視した局所名一致)で照合する設計とした——`drawingml` 名前空間のプレフィックスは `r:id` の `r` ほど実務上固定的ではなく(`a:`が一般的だが仕様上必須ではない)、要素名自体(`dk1`/`lt1`/...)がこのスキーマ内で衝突しない固有の語彙であるため、プレフィックス照合よりも局所名照合の方が安全側に倒れる。この判断が実装時にも妥当か、実際のプレフィックス揺れを持つファイルで再検証する。
2. **`theme{N}.xml` パーツの実体パス解決方式**: 現状ドラフトは `xl/theme/theme1.xml` のような固定パスを前提にしていない(`path` を呼び出し元から受け取る)が、実際に `pipeline.rs` 側でどうパスを解決するか(`xl/_rels/workbook.xml.rels` からのリレーションシップ解決を経由するのが本来のOPC準拠だが、`xl/theme/theme1.xml` を固定パスとして直接読みに行く簡略化もありうる——[pipeline.md オープンクエスチョン3](../pipeline.md) が `workbook.xml` 自体について同種の簡略化からリレーションシップ解決への移行を既に経験済み)は [pipeline.md オープンクエスチョン6](../pipeline.md) として持ち越す。
