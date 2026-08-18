# `resolve/column_width.rs` 設計書

*[English](column_width.en.md)*

`src/resolve/column_width.rs` に対応する設計書。Issue #39 で追加された、フェーズ4の「列幅範囲の検証・登録」を担う。下流の「方眼紙Excel」検出ユースケース([README.md Motivation](../../../README.md#motivation)参照)を実現するため、`<cols>` の範囲リストを検証したうえで [`model::Sheet::set_col_widths`](../model/sheet.md) へ登録する。

本ファイルの構造は意図的に [`resolve/merge.rs`](merge.md) と対になっている: いずれもフェーズ3が収集した範囲のバッチを検証し、1回の呼び出しで `Sheet` へ登録する。両者の違いは元データの形状の違いに起因する——詳細は下記「`resolve/merge.rs` との関係」参照。

## 責務・スコープ

- フェーズ3(`parse/worksheet.rs`)が収集した `<cols>` の範囲リスト(`Vec<model::sheet::ColWidthRange>`)と `default_col_width: Option<f64>`(`<sheetFormatPr defaultColWidth>` 由来)を受け取り、範囲リストを検証したうえで `Sheet::set_col_widths` を1回呼び出す
- `MAX_COLUMN_WIDTH_RANGES` を超えるバッチはソート前に拒否(`Error::TooManyColumnWidthRanges`)し、`min` でソート後に重複する範囲を拒否する(`Error::InvalidColumnWidthRange`)——`resolve/merge.rs` と同じfail closedの方針
- **含まない責務**: `<col min=".." max=".." width=".."/>` 属性集合からの `ColWidthRange` の構築そのもの(`parse/worksheet.rs`)、二分探索のルックアップロジックそのもの(`Sheet::column_width`。[model/sheet.md](../model/sheet.md)参照)

## 主要な型・関数

```rust
use crate::error::Error;
use crate::model::{ColWidthRange, Sheet};

pub(crate) const MAX_COLUMN_WIDTH_RANGES: usize = 2_000;

pub(crate) fn resolve(
    sheet: &mut Sheet,
    mut ranges: Vec<ColWidthRange>,
    default_col_width: Option<f64>,
) -> Result<(), Error> {
    if ranges.len() > MAX_COLUMN_WIDTH_RANGES {
        return Err(Error::TooManyColumnWidthRanges {
            count: ranges.len(),
            limit: MAX_COLUMN_WIDTH_RANGES,
        });
    }

    for range in &ranges {
        if range.min > range.max {
            return Err(Error::InvalidColumnWidthRange {
                min: range.min,
                max: range.max,
                reason: "min must not be greater than max".to_string(),
            });
        }
    }

    ranges.sort_by_key(|r| r.min);
    for pair in ranges.windows(2) {
        let (prev, next) = (&pair[0], &pair[1]);
        if prev.max >= next.min {
            return Err(Error::InvalidColumnWidthRange {
                min: next.min,
                max: next.max,
                reason: "overlaps another column width range".to_string(),
            });
        }
    }

    sheet.set_col_widths(ranges, default_col_width);
    Ok(())
}
```

## `resolve/merge.rs` との関係

いずれもフェーズ3が収集した範囲のバッチを検証してから登録し、いずれも最初の不正なエントリでバッチ全体を拒否する。重複判定の戦略が異なるのは、両者の重複検出問題の形状が異なるためである:

- **`resolve/merge.rs`** は2次元の矩形(`MergedRegion`、行×列)を検証する。新しい範囲を既に受理済みの各範囲と比較するコストは分離軸判定によりペアあたりO(1)だが、あくまで矩形同士の比較なのでバッチ全体ではO(N²)になる。O(N log N)へのスイープライン法への書き換えは検討されたが明示的に見送られており([merge.md オープンクエスチョン2](merge.md)参照)、代わりに防御的な件数上限 `MAX_MERGE_REGIONS` でO(N²)を有界に抑えている。
- **`resolve/column_width.rs`** は1次元の区間(`ColWidthRange`、列のみ)を検証する——構造的により単純な問題である。`min` で1回ソートすればO(R log R)であり、ソート後は**隣接**ペアのみを確認すれば全ての重複ペアを検出するのに十分である: 隣接する全てのペアが `prev.max < next.min` を満たすなら、その関係はソート済みの並び全体に推移的に連鎖するため、隣接しないペアが重複することもあり得ない。これは `resolve/merge.rs` が2次元問題のために採用できなかったO(N log N)のスイープライン形の手法であり、1次元の区間重複判定が単純にソートへ帰着するため「ただで」実現できている。

本モジュール自体にはO(R²)/O(R³)のリスクは無いが、`resolve/merge.rs` と同じ理由で件数上限(`MAX_COLUMN_WIDTH_RANGES`、2,000)を設けている: 最小の `<col min="1" max="1" width=".."/>` エントリはわずか約40〜50バイトのため、Zip Bomb対策のバイト数上限(既定512 MiB)だけでは1,000万件を優に超える数が許容されてしまう——実CPU時間(ソート処理)と実メモリ(範囲1件あたり`Vec<ColWidthRange>`のエントリ)はバイトサイズとは独立に抑えるべきであり、その規模でのソート自体が危険だからではなく、「ファイル形式がそれを止めない」ことは「それを行うのがタダである」ことを意味しないためである。

**設計の経緯**: この考え方——および `resolve/merge.rs` のO(N²)手法や `MAX_MERGE_REGIONS` の値をそのまま流用しなかった判断——は、[Issue #36](https://github.com/MinamiyamaKotaro/xlsxparser/issues/36) の議論スレッドにおける5回にわたる提案設計と実測による反例の結果である: 重複を一切扱わない素朴な案は最悪ケースでO(R³)のリスクがあると実測で判明し、「後勝ち」でのトリム・分割方式は(そのトリム・分割ロジック自体の複雑度リスクが指摘された後)`resolve/merge.rs` 自身の方針に合わせて完全拒否方式に置き換えられ、上限値`2,000`の根拠も2回の訂正を経た(最初は `MAX_MERGE_REGIONS` のdocコメントから当てはまらないO(R²)の理由をそのまま引用し、その後アルゴリズム上の計算量の懸念とは独立に「Zip Bombのバイト数上限だけではRを抑えられない」という正しい理由に訂正)うえで、上記の理由に落ち着いた。

## 依存関係

- 依存先: [`model/sheet.rs`](../model/sheet.md)(`Sheet::set_col_widths`, `ColWidthRange`)、[`error.rs`](../error.md)
- 依存元: [`resolve/mod.rs`](mod.md)(`resolve_sheet` から、`style::resolve` の後・`merge::resolve` の前に呼ばれる)

## エラー処理方針

- 件数が `MAX_COLUMN_WIDTH_RANGES` を超える場合、個々の範囲が `min > max` の場合、または2つの範囲が重複する場合は、それぞれ `Error::TooManyColumnWidthRanges` / `Error::InvalidColumnWidthRange { min, max, reason }` として拒否する(`Error::TooManyMergedRanges` / `Error::InvalidMergedRange` と同じ形——`resolve::merge::validate_region` の座標逆転チェックも含む。[PR #48のレビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/48#pullrequestreview-4956349641)を受けて追加: `min > max` の範囲はそれ自体クラッシュやメモリ安全性の問題は起こさない(`Sheet::column_width` の二分探索がどの列にも一致させないだけ)が、このチェックが無いと、不正な入力をエラーとして表面化させる代わりに死んだ・到達不能なデータとして静かに登録されてしまう)。
- `panic` はしない(不正・悪意ある範囲は信頼できない外部入力に由来しうるため)。
- 検証(件数・逆転範囲・重複のいずれか)に失敗した場合、何も登録しない(fail closed)——`resolve/merge.rs` と同じ方針。

## テスト方針

- 入力順序によらず、非重複の範囲が正しく登録されること(内部でソートされる)の確認
- **`min > max` の範囲が `Error::InvalidColumnWidthRange` として拒否されることの確認(本モジュールのレベル、および `<col min="10" max="5" .../>` フィクスチャによるエンドツーエンドの両方)**(PR #48レビューでの指摘)
- 重複する範囲(完全に同一な重複を含む)が `Error::InvalidColumnWidthRange` として拒否され、その後何も登録されないことの確認
- 隣接するが重複しない範囲(一方の `max` がもう一方の `min - 1` に等しい)が受理されることの確認(隣接ペア判定の境界値テスト)
- 件数がちょうど `MAX_COLUMN_WIDTH_RANGES` の場合は受理され、1件超過すると `Error::TooManyColumnWidthRanges` として拒否されることの確認
- 空の範囲リストでも `default_col_width` が正しく登録されることの確認
- 単一の全幅範囲(`min=1, max=16384`)が列ごとではなく1件として登録されることの確認(Issue #39の核心的なパフォーマンス要件を、本モジュールのレベルで検証)
- エンドツーエンド: `MAX_COLUMN_WIDTH_RANGES + 1` 件の `<col>` を持つファイルが `Error::TooManyColumnWidthRanges` として拒否されること(`pipeline.rs` 自身のテストスイート。`excessive_merge_cell_count_is_too_many_merged_ranges` と同様の形)

## 未決事項 / オープンクエスチョン

現時点で無し。核となるアルゴリズムは上記のIssue #36レビュープロセスを通じて実装着手前に収束した。実装後に見つかった唯一の抜け([`min > max` 検証の欠落](https://github.com/MinamiyamaKotaro/xlsxparser/pull/48#pullrequestreview-4956349641))は未決事項として残さず、既に上記へ反映済み。
