# `model/workbook.rs` 設計書

*[English](workbook.en.md)*

`src/model/workbook.rs` に対応する設計書。全フェーズの解決処理が完了した最終的なデータモデルであり、`lib.rs` の公開API（`parse_workbook(path) -> Result<Workbook>`）の返り値そのものになる。[model/sheet.md](sheet.md) の `Sheet` を集約する。

## 責務・スコープ

- 複数の [`Sheet`](sheet.md) をソース（`xl/workbook.xml` の `<sheets>` 定義順）の順序で保持する
- シート名によるアクセスを提供する
- ワークブックが `theme{N}.xml` パーツを持つ場合、その [`ThemePalette`](color.md) を保持する（[Issue #76](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76)。下記オープンクエスチョン4参照）
- **含まない責務**: `workbook.xml` のXMLパース（`parse/workbook.rs`）、シートIDと実体ファイルパスのルーティング解決（`parse/relationships.rs`。ルーティングマップはフェーズ1完了時に破棄されるため、本モデルには残らない）、`theme{N}.xml` のXMLパースそのもの（[`parse/theme.rs`](../parse/theme.md)）、`ColorRef` から実RGB値への解決ロジック（[`resolve/color.rs`](../resolve/color.md)。本ファイルは構築済みの `ThemePalette` を保持し呼び出し元へ貸し出すのみ）

## 主要な型（案）

```rust
use crate::model::color::ThemePalette;
use crate::model::sheet::Sheet;

/// 解決済みの最終出力モデル。`lib.rs::parse_workbook` の返り値。
#[derive(Debug, Clone)]
pub struct Workbook {
    sheets: Vec<Sheet>,
    /// `xl/theme/theme{N}.xml` パーツから解決した12色のテーマパレット。
    /// パーツ自体を持たないワークブック(テーマカラーを一切使わない
    /// 大多数のファイル)では`None`(Issue #76)。[`resolve::color::resolve_color`](../resolve/color.md)
    /// へ渡すことで、`ResolvedStyle.fill_fg_color`等の`ColorRef::Theme`を
    /// 実RGB値へ解決できる——「案A: オンデマンド解決API」([Issue #76 設計提案](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575))
    /// が機能するために、呼び出し側が`ColorRef`と対になる`ThemePalette`
    /// へアクセスできる経路として、本設計作成時に追加した([model/color.md オープンクエスチョン2](color.md)参照)。
    theme: Option<ThemePalette>,
}

impl Workbook {
    /// 解決済みシートのリストとテーマパレットから構築する。`pipeline.rs` が
    /// 全シートのフェーズ3・4完了後に1回だけ呼び出す（pipeline.md 参照。
    /// 設計時に発見した欠落のため追加）。
    pub(crate) fn new(sheets: Vec<Sheet>, theme: Option<ThemePalette>) -> Self {
        Self { sheets, theme }
    }

    /// ソースファイルでの定義順を維持したシート一覧。
    pub fn sheets(&self) -> &[Sheet] {
        &self.sheets
    }

    /// シート名で検索する。要求仕様書に線形探索を禁止する記述はないため、
    /// シート数が実務上小さい（数〜数十）ことを踏まえ Vec の線形探索で十分と仮定する。
    pub fn sheet(&self, name: &str) -> Option<&Sheet> {
        self.sheets.iter().find(|s| s.name == name)
    }

    /// テーマパレット(存在する場合)。`ColorRef::Theme`を実RGB値へ
    /// 解決する際、[`resolve::color::resolve_color`](../resolve/color.md)
    /// の`theme`引数へそのまま渡す想定。
    pub fn theme(&self) -> Option<&ThemePalette> {
        self.theme.as_ref()
    }
}
```

## 依存関係

- 依存先: [`model/sheet.rs`](sheet.md)、[`model/color.rs`](color.md)（`ThemePalette`。Issue #76）
- 依存元: [`pipeline.rs`](../pipeline.md)（`Workbook::new` で構築・返却。`theme{N}.xml` パーツの読み込み・[`parse/theme.rs`](../parse/theme.md) の呼び出しは [pipeline.md オープンクエスチョン6](../pipeline.md) 参照）、`lib.rs`（公開APIの返り値型）、[`json.rs`](../json.md)（トップレベルのシリアライズ対象。現時点では`theme`をJSON出力へ含めない——[resolve/color.md オープンクエスチョン1](../resolve/color.md)参照）、クレート外部の呼び出し元（`Workbook::theme()` と `ResolvedStyle.fill_fg_color`/`fill_bg_color` を [`resolve::color::resolve_color`](../resolve/color.md) へ渡して実RGB値を得る、Issue #76が想定する表示用途）

## エラー処理方針

- `Workbook` 自体はエラーを生成しない（構築済みの正常データのみを保持する終端モデル）。構築過程（`workbook.xml` の必須要素欠落、シート実体ファイルの参照切れなど）のエラーは `pipeline.rs` / `parse/workbook.rs` 側の責務であり、`error.rs` の共通型を通じて `parse_workbook` の `Result::Err` として呼び出し元に伝播する。
- `sheet(name)` はシートが存在しない場合に `Option::None` を返す（`Result` にしない。名前検索のミスは呼び出し側のロジックエラーであり、ライブラリ内部の異常ではないため）。

## テスト方針

- 複数シートを持つ `Workbook` からの `sheet(name)` 検索（存在する名前／しない名前）
- `sheets()` がソース定義順を保持していることの確認
- シートが0件の `Workbook`（空ブック、または全シート非表示など）に対する `sheets()` / `sheet()` の挙動確認
- `theme{N}.xml` パーツを持つ `Workbook` で `theme()` が `Some(&ThemePalette)` を返すこと、持たない `Workbook` で `None` を返すことの確認(Issue #76)

## 未決事項 / オープンクエスチョン

1. ~~非表示シートの扱い~~ → **解決**: `Workbook.sheets` には可視性を問わず全シートを含める。非表示シートを除外すると他シートからの数式参照解決が破綻しうる、またデータ抽出用途のパーサーとして不完全になるため。可視性は [`Sheet::visibility`](sheet.md)（`SheetVisibility` enum）として保持し、フィルタリングは呼び出し側（または `json.rs`）の任意選択とする（[PR #5 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/5#pullrequestreview-4948235239)を踏まえて確定）。
2. **シート検索の計算量**: シート数が極端に多いブック（数百シート等）を想定する場合、線形探索ではなく `IndexMap` 等への変更を検討する余地がある。現時点では要求仕様書の主眼が「方眼紙Excel（1シート内の行・列が多い）」であり、シート数自体の増大は想定要件に含まれていないため、Vec + 線形探索を仮の設計とした。
3. **ブック全体のメタデータ**: 作成者・作成日時などの `docProps` 系情報は要求仕様書のスコープ外だが、将来的に追加する場合は `Workbook` に直接フィールドを増やすか、別途 `Metadata` 型に分離するかは未決定（現時点ではスコープ外として型に含めない）。
4. **`theme` フィールドの追加は本設計作成時の補完**: [Issue #76 設計提案](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)自体には `Workbook` への変更が明記されておらず、[`resolve/color.md`](../resolve/color.md) の「案A」を実際に呼び出し可能にするために本設計作成時に追加が必要だと判明した。`theme{N}.xml` パーツの実体パス解決・読み込みタイミング(`pipeline.rs` 側の変更)は [pipeline.md オープンクエスチョン6](../pipeline.md) として未解決のまま残っており、`Workbook::new` のシグネチャ変更は実装時にこれと合わせて確定させる。
