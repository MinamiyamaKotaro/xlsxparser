# `resolve/color.rs` 設計書

*[English](color.en.md)*

`src/resolve/color.rs` に対応する設計書。[Issue #76](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76)（テーマカラー/インデックスカラーの実RGB値への変換）が要求する解決ロジックを担う。[`model/style.rs`](../model/style.md) が定義する `ColorRef`(生の色指定)と [`model/color.rs`](../model/color.md) が定義する `ThemePalette`(テーマの12色)から、実際に表示される `Rgb` 値を計算する純粋関数群を提供する。

[architecture.md](../architecture.md) 設計方針2「`resolve/` 配下はI/Oやスタイル解決とは独立し、メモリ上のデータ構造のみで完結させる」に従い、XMLパースは一切行わない——[`parse/theme.rs`](../parse/theme.md)（`ThemePalette` を構築する）とは互いを直接知らず、[`model/color.rs`](../model/color.md) の型のみを介して間接的につながる（[`resolve/style.rs`](style.md) と `parse/styles.rs` の関係と同じ形）。

## 責務・スコープ

- `tint` によるHSL輝度補正を行う純粋関数 `apply_tint` を提供する(sRGB→HSL→輝度補正→sRGBの自己完結した変換。fill色専用に限定せず、将来font色・border色でテーマカラー対応が必要になった場合も使い回せる)
- ECMA-376のレガシー64色固定パレットへの参照を実RGB値へ解決する `lookup_indexed_color` を提供する。`indexed=64`(システム前景色)/`65`(システム背景色)を、OS非依存の決定論的な固定値(`64→#000000`, `65→#FFFFFF`)として解決する([Issue #76コメント](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352388486)の確定仕様。ヘッドレス環境で動くパーサーである本クレートの性質上、OSのシステムパレットには依存できないための判断)
- [`model::style::ColorRef`](../model/style.md) の3バリアント(`Rgb`/`Theme`/`Indexed`)いずれも実RGB値へ解決するエントリポイント `resolve_color` を提供する([Issue #76 設計提案](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)「案A: オンデマンド解決API」の実体。呼び出し元(表示用途のコンシューマ)が必要な箇所でのみ呼び出す想定であり、パース時・スタイル解決時(フェーズ2/4)に無条件で計算することはしない——`ResolvedStyle` 自体のメモリレイアウトは1バイトも変わらない)
- **含まない責務**: `Rgb`/`ThemePalette` の型定義そのもの([`model/color.rs`](../model/color.md))、`ColorRef` の型定義そのもの([`model/style.rs`](../model/style.md))、`theme{N}.xml` のXMLパース([`parse/theme.rs`](../parse/theme.md))、`theme{N}.xml` パーツを読み込むかどうかの判断(`pipeline.rs`。[pipeline.md オープンクエスチョン6](../pipeline.md)参照)

## 主要な型・関数（案）

```rust
use crate::model::color::{Rgb, ThemePalette};
use crate::model::style::ColorRef;

/// ECMA-376のレガシー64色固定パレット(indexed=0..=63)。バイナリに埋め込む
/// コンパイル時定数配列で、実行時メモリ消費は0([Issue #76 設計提案](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575))。
/// PoC検証で、リポジトリ同梱フィクスチャが自身の`<colors><indexedColors>`
/// として再宣言している値、および`openpyxl.styles.colors.COLOR_INDEX`
/// の両方と64/64件一致することを確認済み([Issue #76コメント](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352366260))。
const INDEXED_PALETTE: [Rgb; 64] = [
    // ECMA-376デフォルトの64色。値はPoC検証時に実データと突合済み——
    // 実装時に同じ表をここへ書き起こす(PoCコード自体は`poc/`配下にあり
    // リポジトリには残らないため、実装時に改めて書き下ろす)。
];

/// `theme{N}.xml`の`<clrScheme>`が定義する基準色に`tint`輝度補正を
/// 適用する。`tint`が`0.0`または非有限値(`NaN`/`Inf`)の場合は`base`を
/// そのまま返す——細工された`tint`値(`tint="nan"`等)に対する安全な
/// 縮退([Issue #76 設計提案](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)
/// セキュリティ・堅牢性への配慮)。
///
/// 計算式(ECMA-376の輝度補正アルゴリズム、Apache POIの実装・複数の
/// 独立した情報源で確認済み——[Issue #76コメント](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352366260)):
/// `tint > 0`のとき`l' = l*(1-tint) + tint`(明るくする)、`tint < 0`の
/// とき`l' = l*(1+tint)`(暗くする)。PoCでRust実装とPython `colorsys`
/// による独立した再実装の結果が完全一致することを確認済み
/// (`#4F81BD` + tint -0.25 → `#376092`)。
pub(crate) fn apply_tint(base: Rgb, tint: f64) -> Rgb {
    let _ = (base, tint);
    unimplemented!()
}

/// sRGBをHSLへ変換する。`apply_tint`専用の内部ヘルパー。
fn rgb_to_hsl(c: Rgb) -> (f64, f64, f64) {
    let _ = c;
    unimplemented!()
}

/// HSLをsRGBへ変換する。`apply_tint`専用の内部ヘルパー。
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> Rgb {
    let _ = (h, s, l);
    unimplemented!()
}

/// レガシーインデックスカラー(`indexed`属性)を実RGB値へ解決する。
/// `0..=63`は`INDEXED_PALETTE`への単純な引き当て。`64`/`65`は
/// システム前景色/背景色を表す特殊値で、OS非依存の決定論的な固定色
/// (`64→黒`, `65→白`)として解決する([Issue #76コメント](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352388486))。
/// `66`以上は範囲外としてパニックせず`None`を返す。
pub(crate) fn lookup_indexed_color(index: u32) -> Option<Rgb> {
    match index {
        0..=63 => Some(INDEXED_PALETTE[index as usize]),
        64 => Some(Rgb { r: 0x00, g: 0x00, b: 0x00 }),
        65 => Some(Rgb { r: 0xFF, g: 0xFF, b: 0xFF }),
        _ => None,
    }
}

/// `ColorRef`が指す実RGB値を解決する。`theme`はワークブックが
/// `theme{N}.xml`パーツを持つ場合のみ`Some`([`model::Workbook::theme`](../model/workbook.md)。
/// パーツ自体が存在しないブックでは`None`)。
///
/// - `ColorRef::Rgb(s)`: `s`(8桁ARGB文字列)の下位6桁をRGBとしてパースする。
///   アルファは読み捨てる([model/color.md オープンクエスチョン1](../model/color.md)参照)。
///   `s`が6桁/8桁16進数として不正な場合は`None`(パース時に`ColorRef::Rgb`は
///   値の妥当性を検証せずそのまま保持しているため——[model/style.md](../model/style.md)参照)。
/// - `ColorRef::Theme { index, tint }`: `theme`が`None`(テーマパーツなし)、
///   または`index`が`0..=11`の範囲外の場合は`None`。それ以外は
///   `theme`から基準色を引き、`tint`が`Some`なら`apply_tint`を適用する。
/// - `ColorRef::Indexed(index)`: `lookup_indexed_color`へそのまま委譲する。
///
/// いずれの分岐も`panic`しない——不正・細工された入力に対しては`None`
/// へ安全に縮退する([Issue #76 設計提案](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)
/// セキュリティ・堅牢性への配慮)。
pub fn resolve_color(color: &ColorRef, theme: Option<&ThemePalette>) -> Option<Rgb> {
    let _ = (color, theme);
    unimplemented!()
}
```

`resolve_color`が`ColorRef`自身のインヘレントメソッド(`color.resolve(theme)`)ではなくフリー関数である理由: [Issue #76 設計提案](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)の原案は`ColorRef::resolve(&self, ...)`というインヘレントメソッドを想定していたが、`ColorRef`は[`model/style.rs`](../model/style.md)に定義されており、[architecture.md](../architecture.md)が`model/`に課す「ロジックを持たない純粋データ構造のみ」という制約(すでに[model/mod.md](../model/mod.md)「含まない責務」で明文化されている)に反する。[`resolve/style.rs`](style.md)の`resolve()`が`ResolvedStyle`のインヘレントメソッドではなくフリー関数である既存の設計判断と一貫させ、`model/`の型に一切ロジックを持たせない方針をそのまま踏襲する。

## 依存関係

- 依存先: [`model/color.rs`](../model/color.md)（`Rgb`, `ThemePalette`）、[`model/style.rs`](../model/style.md)（`ColorRef`）
- 依存元: `json.rs`(将来、表示用途のJSON出力が必要になった場合——現時点では呼び出さない。[json.md オープンクエスチョン4](../json.md)参照)、クレート外部の呼び出し元(`resolve_color`は`pub`であり、`Workbook`/`ResolvedStyle`から得た`ColorRef`と`Workbook::theme`を渡して直接呼び出せる想定——`案A`が意図する「必要な箇所でのみ呼び出す」使い方そのもの)

`resolve/mod.rs`の`resolve_sheet`(フェーズ4のエントリポイント)からは呼び出さない——[`resolve/style.rs`](style.md)がセルへ適用する`ColorRef`は生の指定のまま保持され続け、実RGB値への解決はセル走査とは独立した、呼び出し側の任意選択のタイミングで行われる([Issue #76 設計提案](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)「セル単位ではなくスタイル単位/呼び出し時のみ」の方針)。

## エラー処理方針

- `apply_tint`/`lookup_indexed_color`/`resolve_color`はいずれも`Result`を返さず、失敗しうる全ての分岐を`None`(または`apply_tint`の場合は`base`をそのまま返す恒等写像)へ縮退させる——不正・細工された`.xlsx`(範囲外の`theme`インデックス、非有限な`tint`、範囲外の`indexed`値)に対して`panic`しないことを最優先する([Issue #76 設計提案](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)セキュリティ・堅牢性への配慮を実装する)
- この「呼び出し元にエラーとして伝播させず`None`へ縮退する」方針は、[`resolve/style.rs`エラー処理方針](style.md)が採用する「個々の値解釈の緩やかな失敗はドキュメント全体の整合性を損なわない限りエラーにしない」の延長だが、`resolve/style.rs::resolve`とは異なり本ファイルの関数群はそもそも`Result`を返す設計にすらしていない——`resolve_color`はフェーズ4のパイプラインの一部ではなく呼び出し元が任意のタイミングで呼ぶAPIであるため、「解決できなかった」ことをエラーとして扱う理由がない(表示すべき色が特定できなかった、という情報そのものが`None`で十分に表現される)

## テスト方針

- **`apply_tint`**: `tint=0.0`で`base`が変化しないこと、`tint=NaN`/`tint=Infinity`で`base`が変化しないこと(パニックしないこと)、`tint=1.0`で完全に白(`#FFFFFF`)へ、`tint=-1.0`で完全に黒(`#000000`)へ収束すること(境界値)、`accent1(#4F81BD)`に`tint=-0.25`を適用した結果が`#376092`になること(PoCで実データ・独立実装で検証済みの具体値の回帰テスト——[Issue #76コメント](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352366260))
- **`lookup_indexed_color`**: `0`/`63`(範囲の両端)が`INDEXED_PALETTE`の対応する値へ解決されること、`64`が`#000000`へ、`65`が`#FFFFFF`へ解決されること、`66`および`u32::MAX`が`None`へ解決されること(パニックしないこと)
- **`resolve_color`**: `ColorRef::Rgb("FFFF0000")`が`Rgb{r:0xFF,g:0x00,b:0x00}`へ解決されること、不正な16進数文字列を持つ`ColorRef::Rgb`が`None`へ解決されること
- **`resolve_color`**: `ColorRef::Theme{index:4,tint:Some(-0.25)}`が、対応する`ThemePalette`から`apply_tint`を適用した値へ解決されること。`tint:None`の場合は基準色がそのまま返ること
- **`resolve_color`**: `theme:None`(テーマパーツを持たないワークブック)で`ColorRef::Theme`を解決しようとした場合に`None`が返ること(テーマ不在での安全な縮退)
- **`resolve_color`**: `index`が`12`以上の`ColorRef::Theme`が`None`へ解決されること(範囲外インデックスの安全な縮退)
- **`resolve_color`**: `ColorRef::Indexed(64)`/`ColorRef::Indexed(200)`がそれぞれ`lookup_indexed_color`と同じ結果(`Some(#000000)`/`None`)へ解決されること(委譲の結線確認)

## 未決事項 / オープンクエスチョン

1. **`json.rs`での表示用途出力への統合**: [Issue #76 設計提案](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)が「案B」として触れていた、`ResolvedStyle`または`JsonCell`へ解決済みRGB値を含める要件が実際に生じた場合の対応は、具体的な下流ユースケースが現れるまで着手しない。現時点では`resolve_color`をクレート利用者が直接呼び出す「案A」のみを実装する。
2. **`INDEXED_PALETTE`のスタイル**: 上記ドラフトでは値を省略している(PoCコード自体は`poc/`配下にありリポジトリには残らないため)。実装時に[Issue #76コメント](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352366260)のPoCで検証済みの64値をそのまま書き起こす。
