# `model/workbook.rs` 設計書

`src/model/workbook.rs` に対応する設計書。全フェーズの解決処理が完了した最終的なデータモデルであり、`lib.rs` の公開API（`parse_workbook(path) -> Result<Workbook>`）の返り値そのものになる。[model/sheet.md](sheet.md) の `Sheet` を集約する。

## 責務・スコープ

- 複数の [`Sheet`](sheet.md) をソース（`xl/workbook.xml` の `<sheets>` 定義順）の順序で保持する
- シート名によるアクセスを提供する
- **含まない責務**: `workbook.xml` のXMLパース（`parse/workbook.rs`）、シートIDと実体ファイルパスのルーティング解決（`parse/relationships.rs`。ルーティングマップはフェーズ1完了時に破棄されるため、本モデルには残らない）

## 主要な型（案）

```rust
use crate::model::sheet::Sheet;

/// 解決済みの最終出力モデル。`lib.rs::parse_workbook` の返り値。
#[derive(Debug, Clone)]
pub struct Workbook {
    sheets: Vec<Sheet>,
}

impl Workbook {
    /// ソースファイルでの定義順を維持したシート一覧。
    pub fn sheets(&self) -> &[Sheet] {
        &self.sheets
    }

    /// シート名で検索する。要求仕様書に線形探索を禁止する記述はないため、
    /// シート数が実務上小さい（数〜数十）ことを踏まえ Vec の線形探索で十分と仮定する。
    pub fn sheet(&self, name: &str) -> Option<&Sheet> {
        self.sheets.iter().find(|s| s.name == name)
    }
}
```

## 依存関係

- 依存先: [`model/sheet.rs`](sheet.md)
- 依存元: `pipeline.rs`（構築・返却）、`lib.rs`（公開APIの返り値型）、`json.rs`（トップレベルのシリアライズ対象）

## エラー処理方針

- `Workbook` 自体はエラーを生成しない（構築済みの正常データのみを保持する終端モデル）。構築過程（`workbook.xml` の必須要素欠落、シート実体ファイルの参照切れなど）のエラーは `pipeline.rs` / `parse/workbook.rs` 側の責務であり、`error.rs` の共通型を通じて `parse_workbook` の `Result::Err` として呼び出し元に伝播する。
- `sheet(name)` はシートが存在しない場合に `Option::None` を返す（`Result` にしない。名前検索のミスは呼び出し側のロジックエラーであり、ライブラリ内部の異常ではないため）。

## テスト方針

- 複数シートを持つ `Workbook` からの `sheet(name)` 検索（存在する名前／しない名前）
- `sheets()` がソース定義順を保持していることの確認
- シートが0件の `Workbook`（空ブック、または全シート非表示など）に対する `sheets()` / `sheet()` の挙動確認

## 未決事項 / オープンクエスチョン

1. **非表示シートの扱い**: `workbook.xml` の `<sheet>` 要素は `state="hidden"` / `"veryHidden"` を持ちうる。`Workbook.sheets` に可視性を問わず全件含めるか、可視シートのみを含めるか、あるいは `Sheet` 側に可視性フィールドを持たせて呼び出し側でフィルタさせるかは未決定。要求仕様書に明記がないため確認が必要。
2. **シート検索の計算量**: シート数が極端に多いブック（数百シート等）を想定する場合、線形探索ではなく `IndexMap` 等への変更を検討する余地がある。現時点では要求仕様書の主眼が「方眼紙Excel（1シート内の行・列が多い）」であり、シート数自体の増大は想定要件に含まれていないため、Vec + 線形探索を仮の設計とした。
3. **ブック全体のメタデータ**: 作成者・作成日時などの `docProps` 系情報は要求仕様書のスコープ外だが、将来的に追加する場合は `Workbook` に直接フィールドを増やすか、別途 `Metadata` 型に分離するかは未決定（現時点ではスコープ外として型に含めない）。
