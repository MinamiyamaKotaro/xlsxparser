# `resolve/row_height.rs` 設計書

*[English](row_height.en.md)*

`src/resolve/row_height.rs` に対応する設計書。姉妹プロジェクトexceldiffのIssue #51 で追加された、フェーズ4の「行高範囲の検証・登録」を担う。[`resolve/column_width.rs`](column_width.md)の行軸版。exceldiffでは`grid.rs`が生成するグリッドHTMLの行の高さを実際のExcelファイルの値に忠実にするために追加されたが、本クレートには`grid.rs`に相当するものが存在しないため、目的は`Sheet::row_height`/`json.rs`の`rows`配列を通じて行高情報をクレート利用者へ公開することに限られる。

本ファイルの構造は`resolve/column_width.rs`と意図的に対になっているが、**元データの形状**が異なるため、圧縮そのものの責務の置き場所が異なる——詳細は下記「`resolve/column_width.rs`との関係」参照。

## 責務・スコープ

- フェーズ3(`parse/worksheet.rs`)が`<sheetData>`をストリーム処理しながら**既に圧縮済み**で収集した行高範囲リスト(`Vec<model::sheet::RowHeightRange>`)と`default_row_height: Option<f64>`(`<sheetFormatPr defaultRowHeight>`由来)を受け取り、検証したうえで`Sheet::set_row_heights`を1回呼び出す
- `MAX_ROW_HEIGHT_RANGES`を超えるバッチはソート前に拒否(`Error::TooManyRowHeightRanges`)し、`min`でソート後に重複する範囲を拒否する(`Error::InvalidRowHeightRange`)——`resolve/column_width.rs`と同じfail closedの方針
- **含まない責務**: `<row r="N" ht="..">`属性からの`RowHeightRange`の構築・圧縮そのもの(`parse/worksheet.rs`の`push_row_height`——`resolve/column_width.rs`との決定的な違い、下記参照)、二分探索のルックアップロジックそのもの(`Sheet::row_height`。[model/sheet.md](../model/sheet.md)参照)

## 主要な型・関数

```rust
use crate::error::Error;
use crate::model::{RowHeightRange, Sheet};

pub(crate) const MAX_ROW_HEIGHT_RANGES: usize = 2_000;

pub(crate) fn resolve(
    sheet: &mut Sheet,
    mut ranges: Vec<RowHeightRange>,
    default_row_height: Option<f64>,
) -> Result<(), Error> {
    if ranges.len() > MAX_ROW_HEIGHT_RANGES {
        return Err(Error::TooManyRowHeightRanges {
            count: ranges.len(),
            limit: MAX_ROW_HEIGHT_RANGES,
        });
    }

    ranges.sort_by_key(|r| r.min);
    for pair in ranges.windows(2) {
        let (prev, next) = (&pair[0], &pair[1]);
        if prev.max >= next.min {
            return Err(Error::InvalidRowHeightRange {
                min: next.min,
                max: next.max,
                reason: "overlaps another row height range".to_string(),
            });
        }
    }

    sheet.set_row_heights(ranges, default_row_height);
    Ok(())
}
```

`resolve::column_width::resolve`と異なり、個々の範囲に対する`min > max`チェックが無い。`parse/worksheet.rs`の`push_row_height`は常に`RowHeightRange { min: row, max: row, .. }`(min==max)から始めて`max`を単調に伸長するだけなので、単一の範囲が`min > max`になることは構造上あり得ない——`<col min="10" max="5">`のように任意の属性値をファイルがそのまま指定できる列幅とは異なる。

## `resolve/column_width.rs`との関係

いずれもフェーズ3が収集した範囲のバッチを検証してから登録し、ソート後の隣接ペアのみを比較することでO(R log R)の重複検出を実現している点は共通(1次元区間の重複判定なので`resolve/merge.rs`のO(N²)矩形判定は不要——理由は[column_width.md](column_width.md)参照)。

決定的に異なるのは**圧縮の責務がどちらの層にあるか**である:

- **列幅**: `<col min=".." max="..">`はOOXMLのスキーマ自体が範囲として表現できるため、`parse/worksheet.rs`はファイルに書かれた範囲をそのまま`ColWidthRange`へ詰め替えるだけで、圧縮そのものは発生しない(ファイル側が既に圧縮済みで渡してくれる)。
- **行高**: `<row r="N" ht="..">`は必ず1行につき1要素で、範囲という概念がスキーマに存在しない。実データ(`.xlsx`のスキルシートテンプレート)では1,000個の個別`<row ht="..">`要素が存在したが、圧縮すればわずか32レンジまで縮小できた(31.2倍)——つまり**圧縮しないと列幅とは比較にならない数のエントリが発生しうる**。この圧縮を`resolve/row_height.rs`側(生の`(row, height_pt)`のペアを受け取ってから圧縮)で行うと、圧縮前の中間バッファが行数に比例して肥大化する(実測: 100万行で約15.6MB)。そのため`parse/worksheet.rs`が`<sheetData>`をストリーム処理する**その場で**圧縮する設計を採用した(`push_row_height`: 直前のレンジの`max`が現在行の直前で高さが同じなら伸長、そうでなければ新しいレンジを開始)。これにより`resolve::row_height::resolve`が受け取る時点で既に圧縮済みのレンジ列になり、本モジュール自体は列幅と同じ検証・登録のみに専念できる(実測: 同じ100万行・単一高さのケースでストリーミング圧縮なら数十バイトで完結)。

この設計判断(バッファリング方式ではなくストリーミング圧縮)は、姉妹プロジェクトexceldiff側の[Issue #51](https://github.com/MinamiyamaKotaro/exceldiff/issues/51)のPoC検証(実ヒープアロケーション計測込み)を経て確定し、`model/sheet.rs`・`parse/worksheet.rs`と同様に本クレートへもそのまま移植した。

## 依存関係

- 依存先: [`model/sheet.rs`](../model/sheet.md)(`Sheet::set_row_heights`, `RowHeightRange`)、[`error.rs`](../error.md)
- 依存元: [`resolve/mod.rs`](mod.md)(`resolve_sheet`から、`column_width::resolve`の後・`merge::resolve`の前に呼ばれる)、[`parse/worksheet.rs`](../parse/worksheet.md)(圧縮済みの`row_height_ranges`/`default_row_height`を`WorksheetParseOutput`として供給する側)

## エラー処理方針

- 件数が`MAX_ROW_HEIGHT_RANGES`を超える場合、または2つの範囲が重複する場合は、それぞれ`Error::TooManyRowHeightRanges` / `Error::InvalidRowHeightRange { min, max, reason }`として拒否する。重複は通常発生しない(`push_row_height`が単一の forward パスで構築するため既にソート済み・非重複)が、ファイルの`<row>`要素が昇順`r`で書かれていない場合(規格違反・悪意ある入力)に備えた防御的チェック。
- `panic`はしない。
- 検証に失敗した場合、何も登録しない(fail closed)——`resolve/column_width.rs`と同じ方針。

## テスト方針

- 入力順序によらず、非重複の範囲が正しく登録されること(内部でソートされる)の確認
- 重複する範囲(完全に同一な重複を含む)が`Error::InvalidRowHeightRange`として拒否されることの確認
- 隣接するが重複しない範囲が受理されることの確認
- 件数がちょうど`MAX_ROW_HEIGHT_RANGES`の場合は受理され、1件超過すると`Error::TooManyRowHeightRanges`として拒否されることの確認
- 空の範囲リストでも`default_row_height`が正しく登録されること、範囲も既定値も無ければ`None`を返すことの確認
- `parse/worksheet.rs`側(圧縮そのもの): 連続する同じ高さの行が1レンジに圧縮されること、行番号が連続しない(間に`<row ht>`の無い行がある)場合は同じ高さでも新しいレンジになること、`ht`属性の無い行は何も生成しないことの確認

## 未決事項 / オープンクエスチョン

1. ~~**JSON出力(`json.rs`)への公開**~~ **解決済み**: 列幅(`col_width_ranges()`/`default_col_width()`)と対称に`row_height_ranges()`/`default_row_height()`を公開し、`json.rs`がシート単位の`rows`/`defaultRowHeight`として(セルごとに複製せず)シリアライズするようにした。
2. **`customHeight`属性の扱い**: `<row ht="..">`は`customHeight="1"`(ユーザーが明示的に設定した高さ)かどうかに関わらず読む方針とした——`customHeight`が無い`ht`はExcel側の自動計算による推定値だが、いずれにせよExcelが実際に描画する高さそのものであり、本ライブラリは「ユーザーが選んだもの」ではなく「見た目の再現」を目的としているため、区別なく採用する。
