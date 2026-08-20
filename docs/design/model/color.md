# `model/color.rs` 設計書

*[English](color.en.md)*

`src/model/color.rs` に対応する設計書。[Issue #76](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76)（テーマカラー/インデックスカラーの実RGB値への変換、表示用途）が必要とする、実RGB値そのものを表す型を定義する。[`model/style.rs`](style.md) が定義する `ColorRef`（`rgb`/`theme`+`tint`/`indexed` の生の指定を保持するのみで、実RGB値へは解決しない——[Issue #75](https://github.com/MinamiyamaKotaro/xlsxparser/issues/75)のdiff指向スコープ）とは独立した、表示用途のためだけの新設ファイル。ロジックを持たない純粋なデータ構造のみを定義する（[model/style.md](style.md) と同じ位置づけ）。

[`parse/theme.rs`](../parse/theme.md)（`theme{N}.xml` から `ThemePalette` を構築する主体）と [`resolve/color.rs`](../resolve/color.md)（`ColorRef` と `ThemePalette` から実RGB値を解決する主体）が、互いを直接知ることなく本ファイルの型だけを介して間接的につながる——[model/style.md](style.md) が `parse/styles.rs`/`resolve/style.rs` 間で果たしているのと同じ「フェーズ間の共有語彙」の役割。

## 責務・スコープ

- 実RGB値を表す軽量な `Copy` 型 `Rgb` を定義する
- `theme{N}.xml` の `<clrScheme>`（12色）を保持する `ThemePalette` を定義する
- **含まない責務**: `theme{N}.xml` のXMLパースそのもの（[`parse/theme.rs`](../parse/theme.md)、未設計）、`ColorRef` から `Rgb` への解決ロジック（tint補正・インデックスパレット引き当てを含む。[`resolve/color.rs`](../resolve/color.md)、未設計）、`ColorRef` 自体の型定義（[`model/style.rs`](style.md)）

## 主要な型（案）

```rust
/// 実RGB値。ヒープアロケーションなしの4バイト境界に収まる `Copy` 型。
/// アルファチャンネルは持たない——セルの塗りつぶしが実際に見えるか
/// どうかは `patternType`(`none`/`solid`等)が制御しており、色そのものに
/// 透過度の概念はほぼ関与しない。`ColorRef::Rgb`(`"FFFF0000"` のような
/// 8桁ARGB文字列)の先頭2桁も実務上ほぼ常に`FF`(不透明)であり、
/// [`resolve/color.rs`](../resolve/color.md) がこれを `Rgb` へ解決する際は
/// 単に読み捨てる(下記オープンクエスチョン1参照)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// `theme{N}.xml` の `<clrScheme>` が定義する12色。スタック上に置ける
/// 固定長配列で保持し、ヒープ確保を一切伴わない（[Issue #76 設計提案](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)
/// が最優先方針とする「パース性能を落とさず、メモリリソースを増やさない」
/// をそのまま反映）。
///
/// **要注意**: 配列のインデックスは `<clrScheme>` のXML宣言順
/// (`dk1, lt1, dk2, lt2, accent1..6, hlink, folHlink`)そのものでは
/// **ない**。`styles.xml` の `theme` 属性が参照するインデックスは
/// スロット0/1が入れ替わった `lt1, dk1, lt2, dk2, accent1..6, hlink,
/// folHlink` の順(Apache POIの`ThemesTable.ThemeElement` enumが採用する
/// 順序と一致し、PoC検証で実データに対して確認済み——[Issue #76コメント](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352366260)参照)。
/// 実装を誤りやすい既知の罠であるため、[`parse/theme.rs`](../parse/theme.md)
/// はこのスワップを吸収した上で本配列を構築する責務を持つ——本ファイル
/// 自身はこのインデックス規約を「配列がそういう順序で格納されている」
/// という契約として文書化するのみで、スワップを行うロジックは持たない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemePalette(pub [Rgb; 12]);
```

`Rgb` が `Default`(黒 `#000000`)を導出している理由は [`resolve/color.rs`](../resolve/color.md) 側の一部フォールバック経路（[Issue #76コメント](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352388486) の `sysClr` フォールバック方針）が「黒」をデフォルト値として使うため。`ThemePalette` は12スロット全てが埋まって初めて意味を持つため `Default` を導出しない——空/部分初期化の `ThemePalette` を作れてしまうと `resolve/color.rs` 側の呼び出し規約が曖昧になる。

## 依存関係

- 依存先: なし（`model/` 内の兄弟モジュールにも依存しない、[`model/style.rs`](style.md) と同じリーフモジュール）
- 依存元: [`parse/theme.rs`](../parse/theme.md)（`ThemePalette` を構築する）、[`resolve/color.rs`](../resolve/color.md)（`Rgb`/`ThemePalette` を読み、`ColorRef` から `Rgb` を解決する）、[`model/workbook.rs`](workbook.md)（`Workbook.theme: Option<ThemePalette>` として保持する——下記オープンクエスチョン2、および [workbook.md](workbook.md) 参照）

## エラー処理方針

対象なし（[`model/style.rs`](style.md) 同様、ロジックを持たない純粋データ構造の定義のみ）。`theme{N}.xml` のパース失敗・スロット欠落に対するエラー化は [`parse/theme.rs`](../parse/theme.md) の責務。

## テスト方針

対象なし。型定義のみのためユニットテストを持たない。`ThemePalette` のインデックス規約（スロット0/1のスワップ）が実際に正しく守られているかの検証は [`parse/theme.rs` テスト方針](../parse/theme.md) 側で行う。

## 未決事項 / オープンクエスチョン

1. **`ColorRef::Rgb` の8桁ARGB文字列からアルファチャンネルを読み捨てる設計の妥当性**: 実務上ほぼ常に `FF`(不透明)であることをPoCで確認したフィクスチャでは検証済みだが、`FF` 以外のアルファ値を持つ実ファイルが将来見つかった場合、それを黙って無視してよいか(現行方針)、`Rgb` にアルファフィールドを追加すべきかは再検討の余地がある。追加コストは低い(`Rgb` はまだどこにも公開APIとして固まっていない)ため、具体的な実例が見つかってから判断する。
2. **`Workbook` への `ThemePalette` の持たせ方**: [Issue #76 設計提案](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)の「案A(オンデマンド解決API)」を機能させるには、呼び出し側が `ColorRef` に加えてワークブックの `ThemePalette` へアクセスできる必要がある——原提案のモジュール構成にはこの経路が明示されていなかったため、本設計作成時に [`model/workbook.rs`](workbook.md) へ `theme: Option<ThemePalette>` フィールドを追加する形で補った(詳細は [workbook.md](workbook.md) 参照)。`theme{N}.xml` パーツを持たないワークブック(テーマカラーを一切使わない大多数のファイル)を表すために `Option` とした。
