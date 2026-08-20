# `parse/worksheet.rs` 設計書

*[English](worksheet.en.md)*

`src/parse/worksheet.rs` に対応する設計書。[architecture.md](../architecture.md) が定義するフェーズ3「`sheetX.xml` のSAXストリームパース（行単位の破棄はここで完結）」そのものを担う。要求仕様書3章のコア機能（疎行列によるメモリ最適化、結合セルの透過的アクセス）を満たすため、[`model::Sheet::insert_cell`](../model/sheet.md) へセルをストリームで挿入しつつ、共有文字列・スタイル・結合範囲という遅延解決が必要な情報を[`resolve/`](../resolve/mod.md)（フェーズ4）へ引き渡す形へ整形する。[resolve/mod.md](../resolve/mod.md)・[resolve/shared_strings.md](../resolve/shared_strings.md)・[resolve/style.md](../resolve/style.md)・[resolve/merge.md](../resolve/merge.md) の4ファイルがいずれも「`parse/worksheet.rs` は未設計」として前提していた契約を、本ファイルで確定させる。

## 責務・スコープ

- `xl/worksheets/sheetX.xml` の `<sheetData>` を行（`<row>`）単位でストリーム処理し、`<c>`（セル）ごとに [`Cell`](../model/cell.md) を構築して `Sheet::insert_cell` で挿入する
- 1行分のデータ（当該行に属する全 `<c>` の読み取りと `insert_cell` への反映）が完了した時点で、その行に関するパーサーの内部状態（属性・テキストバッファ等）を破棄し、次の行の処理へ移る（要求仕様書フェーズ3要件の実装。architecture.md 「行単位のXMLノード破棄は `parse/worksheet.rs` 内部の実装詳細であり、`pipeline.rs` はこれを制御しない」）
- `t="s"`（共有文字列インデックス参照）セルを検出した場合、`value: None` の `Cell` を `insert_cell` で挿入すると同時に、対応する `PendingSharedString`（本ファイルが定義。[`resolve/shared_strings.rs`](../resolve/shared_strings.md) が消費する）を記録する
- `s`（`cellXfs` インデックス。スタイルID参照）属性を持つセルを検出した場合、対応する `PendingStyle`（本ファイルが定義。[`resolve/style.rs`](../resolve/style.md) が消費する）を記録する
- `t="str"`（数式の計算結果文字列）・`t="inlineStr"`（インラインストリング）セルは遅延解決を必要としないため、ストリーム中に直接 `CellValue::Text` として解決し `insert_cell` で挿入する（[resolve/shared_strings.md 責務・スコープ](../resolve/shared_strings.md) が既に前提としていた分担）
- ストリーム完了後（`</sheetData>` の後）に出現する `<mergeCells><mergeCell ref="A1:C3"/>...</mergeCells>` を [`CellRef::from_a1`](../model/cell.md) で `start`/`end` に変換し、`Vec<MergedRegion>` として収集する（[`resolve/merge.rs`](../resolve/merge.md) が検証・登録する前段）
- `<sheetData>` より**前**（OOXMLスキーマ上固定の要素順）に出現する `<cols><col min=".." max=".." width=".."/>...</cols>` を `Vec<ColWidthRange>` として収集する——`width` 属性を実際に持つ `<col>` のみが対象(`hidden`/`bestFit` などしか持たない `<col>` は本ファイルがまだ何も追跡していないためスキップする)。`<sheetFormatPr defaultColWidth="..">` があれば併せて収集する。いずれも [`resolve/column_width.rs`](../resolve/column_width.md) が検証・登録する前段(Issue #39。`<mergeCells>` と同じ2段階の分担)
- `<mergeCells>` と同様 `<sheetData>` の後に出現する `<hyperlinks><hyperlink ref="A1" r:id="rId1" location=".." tooltip=".."/>...</hyperlinks>`(Issue #95)を `Vec<PendingHyperlink>` として収集する——`ref` は単一座標または `start:end` 範囲のいずれかとしてパースし(両方とも `CellRef` で保持。順序の検証はここでは行わず、`<mergeCells>` と同じ分担)、`r:id`/`location`/`tooltip` はそのまま保持する。`pipeline.rs` のフェーズ3.5(`r:id` → 生のTarget文字列。ZIP I/Oが必要)を経て [`resolve/hyperlink.rs`](../resolve/hyperlink.md) の検証・登録へつながる
- **含まない責務**: 共有文字列インデックス・スタイルIDの実際の解決（[`resolve/shared_strings.rs`](../resolve/shared_strings.md) / [`resolve/style.rs`](../resolve/style.md)）、結合範囲・ハイパーリンク範囲・列幅範囲の妥当性検証・`Sheet` への登録そのもの（[`resolve/merge.rs`](../resolve/merge.md) / [`resolve/hyperlink.rs`](../resolve/hyperlink.md) / [`resolve/column_width.rs`](../resolve/column_width.md)。本ファイルは生のリストを収集するのみで `insert_merge`/`finalize_hyperlinks`/`set_col_widths` は呼ばない）、ハイパーリンクの `r:id` を `_rels` に対して解決すること(`pipeline.rs`。ZIP I/Oが必要)、数式（`<f>` 要素）の解析・保持（オープンクエスチョン2参照）

## 主要な型・関数（案）

```rust
use crate::error::Error;
use crate::model::cell::{Cell, CellRef, CellValue};
use crate::model::sheet::{MergedRegion, Sheet};
use crate::model::style::StyleId;
use crate::parse::{concat_rich_text, convert_xml_error, create_secure_reader, required_attr};
use std::io::BufRead;
use std::sync::Arc;

/// フェーズ3が `t="s"` セルを検出した時点で記録する保留エントリ。
/// `model::CellValue` は解決済みの `Text(Arc<str>)` のみを許容しインデックスを
/// そのまま保持するバリアントを持たないため（[model/cell.md](../model/cell.md)）、
/// パース時点ではセル自体を `value: None` のまま `Sheet` へ挿入し（スタイル等の
/// 他フィールドは通常通り設定する）、インデックスは本構造体としてシートの
/// 外側に保持しておく。[`resolve/shared_strings.rs`](../resolve/shared_strings.md)
/// がこれを消費して実文字列へ解決する（[PR #9 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204)
/// を反映し、フェーズ3の出力データそのものとして本ファイルへ定義を移設した。
/// 経緯は依存関係セクション参照）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingSharedString {
    pub cell_ref: CellRef,
    pub index: usize,
}

/// フェーズ3が `s`（style index）属性を持つセルを検出した時点で記録する
/// 保留エントリ。[`resolve/style.rs`](../resolve/style.md) がこれを消費して
/// `ResolvedStyle` を適用する（配置の経緯は `PendingSharedString` と同様。
/// 依存関係セクション参照）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingStyle {
    pub cell_ref: CellRef,
    pub style_id: StyleId,
}

/// `parse_worksheet` の出力。`sheet` 自体は `&mut` 引数で直接書き換えるため、
/// フェーズ4（[resolve/mod.rs](../resolve/mod.md) の `resolve_sheet`）がそのまま
/// 必要とする残り3つの未解決データのみを返す。
pub(crate) struct WorksheetParseOutput {
    pub pending_shared_strings: Vec<PendingSharedString>,
    pub pending_styles: Vec<PendingStyle>,
    pub merge_regions: Vec<MergedRegion>,
}

/// フェーズ3のエントリ関数。`sheet` は `pipeline.rs` が
/// [`parse/workbook.rs`](workbook.md) の結果から `name`/`visibility` を
/// 設定してあらかじめ構築済みのものを受け取り、セルをストリームで挿入していく。
///
/// 呼び出し前提（[resolve/mod.md 呼び出し前提](../resolve/mod.md) と対になる、
/// 本ファイル側が守る契約）:
/// - `t="s"` セルを検出した場合、`value: None` の `Cell` の `insert_cell` と
///   対応する `PendingSharedString` の記録を必ず対で行う
///   （[resolve/shared_strings.md オープンクエスチョン2](../resolve/shared_strings.md) を解決）。
/// - `s` 属性を持つセルを検出した場合、`insert_cell` と対応する
///   `PendingStyle` の記録を必ず対で行う。
///   [`resolve/shared_strings.rs`](../resolve/shared_strings.md) と
///   [`resolve/style.rs`](../resolve/style.md) はこの不変条件が守られている
///   前提で `Sheet::get_mut` の結果を `expect` している。
///
/// `date1904` は `workbook.xml` の `<workbookPr date1904="1"/>` フラグで、
/// 本来はフェーズ4([`resolve/style.rs`](../resolve/style.md))だけの関心事
/// だが、`t="d"` の時刻のみセル(ISO 8601の元テキストに日付部分が無い)の
/// 仮の日付を、同じブック内の数値ベース時刻のみセルが解決する日付と
/// 一致させるためだけにフェーズ3へも運ぶ(PR #80レビュー指摘2。詳細は
/// `parse_iso8601_datetime` のdocコメント参照)。
///
/// `<row>`/`<c>` の `r` 属性(セル参照)はいずれもECMA-376仕様上省略可能で、
/// 省略時は「直前の行/セルの次」として暗黙的に位置推論することが許容
/// されている(Issue #79)。本関数はこの推論に必要な「現在の行番号」
/// 「行内での現在の列位置」をループのローカル状態として保持し、`<row>`
/// の開始タグで行番号を確定・列位置をリセットし、各 `<c>` の開始タグで
/// `r` があればそれを採用しつつ現在列を更新、無ければ現在列を1進めて
/// 採用する。いずれの経路でも [`model/cell.rs`](../model/cell.md) の
/// `CellRef::MAX_ROW`/`MAX_COL` と同じ上限チェックを行う——`r` を省略した
/// `<c>` を大量に並べるだけで `CellRef::from_a1` の上限チェックを経由せず
/// `Sheet::max_col` を膨張させられてしまうため(セキュリティレビュー
/// `docs/security/code-review.md` Finding 2 と同じ攻撃面)。
pub(crate) fn parse_worksheet(
    reader: impl BufRead,
    path: &str,
    sheet: &mut Sheet,
    date1904: bool,
    max_cells: usize,
) -> Result<WorksheetParseOutput, Error> {
    let mut xml_reader = create_secure_reader(reader);
    let mut pending_shared_strings = Vec::new();
    let mut pending_styles = Vec::new();
    let mut merge_regions = Vec::new();
    // 実装方針: 状態機械として「<sheetData>外」「<row>内」「<c>内
    // （t/s属性を保持し、<v>/<is>のテキストを待つ）」を遷移する。
    // <row>のEnd イベントの時点で当該行のスクラッチ状態（開いていた
    // 属性・テキストバッファ）をクリアし、次の<row>のStartへ備える。
    // quick-xmlのread_event_into(&mut buf)自体もbufを呼び出しごとに
    // クリアして再利用するため、複数行分のXMLノードが同時にヒープ上へ
    // 蓄積されることはない。
    let _ = (
        &mut xml_reader,
        path,
        sheet,
        &mut pending_shared_strings,
        &mut pending_styles,
        &mut merge_regions,
    );
    unimplemented!()
}

/// `<c t="...">` の `t` 属性（省略時はNumber相当）に応じて `<v>`/`<is>` の
/// 内容から `Cell` を構築する。`t="s"`/`s`属性ありセルは `value`/`style` を
/// `None` のまま返し、呼び出し元（`parse_worksheet`）が対応する `Pending*`
/// を記録する。
fn build_cell(
    cell_ref: CellRef,
    cell_type: Option<&str>,
    style_id: Option<u32>,
    value_text: Option<&str>,
    inline_string: Option<String>,
    date1904: bool,
) -> Result<Cell, Error> {
    let value = match cell_type {
        None | Some("n") => value_text.map(parse_number).transpose()?.map(CellValue::Number),
        Some("s") => None, // resolve/shared_strings.rs が解決する（PendingSharedStringとして別途記録）
        Some("str") => value_text.map(|s| CellValue::Text(Arc::from(s))),
        Some("inlineStr") => inline_string.map(|s| CellValue::Text(Arc::from(s))),
        Some("b") => value_text.map(|s| s == "1").map(CellValue::Boolean),
        Some("e") => value_text.map(|s| CellValue::Error(s.to_string())),
        // ECMA-376 Part 1のt="d"拡張(Issue #58): <v>のテキストはシリアル値
        // ではなくISO 8601文字列そのもの。
        Some("d") => value_text
            .map(|s| parse_iso8601_datetime(s, date1904))
            .transpose()?
            .map(CellValue::DateTime),
        // 未知のt値: データを取りこぼさないよう生テキストをそのままTextとして
        // 保持するフォールバック（オープンクエスチョン3参照）。
        Some(_) => value_text.map(|s| CellValue::Text(Arc::from(s))),
    };
    let _ = (cell_ref, style_id);
    Ok(Cell { value, style: None })
}

/// `t="d"` セルの `<v>` テキストをISO 8601としてパースする(Issue #58)。
/// 実在ファイル(`tests/fixtures/other/date_iso.xlsx`、calamineテスト
/// コーパス由来)で確認できた3パターン——日付のみ(`2021-01-01`)、
/// 日付+時刻(`2021-01-01T10:10:10`)、時刻のみ(`10:10:10`)——に加え、
/// PR #80レビュー指摘1を受けて以下の仕様準拠のバリエーションも許容する:
/// - 末尾のUTC/オフセット指定子(`Z`、`+09:00`、`-0500`)は読み捨てる。
///   `DateTimeValue` にタイムゾーン用フィールドが無い(Excel自身の日付
///   システム自体がタイムゾーンを持たないのと同じ)ため、オフセットに
///   応じた変換はせず壁時計時刻をそのまま採用する
/// - 小数秒(`10:10:10.500`)は整数秒へ切り捨てる
/// - 秒は省略可(`10:10`)。省略時は0扱い
///
/// これら以外の不正な形式(セグメント数不一致・範囲外の数値等)は
/// これまでどおりエラーとする。
///
/// 時刻のみの値は元テキストに日付部分が無いため、Excel自身の「日付なし
/// 時刻」慣例(シリアル日0)へ倒す——具体的な日付は `date1904` に応じて
/// 1899-12-30(1900年システム)/1904-01-01(1904年システム)のいずれか
/// になる。[`resolve/style.rs`](../resolve/style.md)の `serial_to_date_time`
/// が1未満のシリアル値をすでに同じ規約(`EPOCH_OFFSET_1900`/
/// `EPOCH_OFFSET_1904`)でデコードしているのと整合させたもの(PR #80
/// レビュー指摘2)。`date1904` 自体は本来フェーズ4だけの関心事だが、この
/// 整合を取るためだけに `parse_worksheet` からフェーズ3全体を貫通させて
/// ここまで運ばれてくる(型としては `resolve/style.rs` のプライベートな
/// `EPOCH_OFFSET_1904` 等をフェーズ3から参照することはできない——
/// `parse/` は `resolve/` に依存しないという architecture.md 設計方針2の
/// 制約——ため、同じ2つの日付をこのファイル内で直接ハードコードして
/// いる。両ファイルの記述が食い違わないよう、変更時は双方を確認する
/// こと)。
fn parse_iso8601_datetime(text: &str, date1904: bool) -> Result<DateTimeValue, Error> {
    let _ = (text, date1904);
    unimplemented!()
}

fn parse_number(text: &str) -> Result<f64, Error> {
    let _ = text;
    unimplemented!()
}
```

## 依存関係

- 依存先: [`parse/mod.rs`](mod.md)（`create_secure_reader`, `convert_xml_error`, `required_attr`, `optional_attr`, `concat_rich_text`）、[`model/cell.rs`](../model/cell.md)（`Cell`, `CellRef`, `CellValue`, `DateTimeValue`。`DateTimeValue` は `t="d"` 対応(Issue #58)で新規に依存した——[Issue #40](https://github.com/MinamiyamaKotaro/xlsxparser/issues/40)で導入された型を、シリアル値経由(`resolve/style.rs`)とは独立に本ファイルからも直接構築する2つ目の経路になる）、[`model/sheet.rs`](../model/sheet.md)（`Sheet::insert_cell`, `MergedRegion`）、[`model/style.rs`](../model/style.md)（`StyleId`。`PendingStyle` のフィールド型として使う）、[`error.rs`](../error.md)。`resolve/` 配下のいずれのモジュールにも依存しない
- 依存元: `pipeline.rs`（フェーズ3。シートごとに1回呼び出し、返り値を [`resolve::resolve_sheet`](../resolve/mod.md) へそのまま渡す。加えて `pending_hyperlinks` は別途、自身のフェーズ3.5/`resolve::hyperlink::resolve` の経路へ渡す——[pipeline.md](../pipeline.md) 参照。`date1904`(`t="d"` 対応、Issue #58/PR #80レビュー指摘2で新規に追加した引数)は `pipeline.rs` がフェーズ1で `parse_workbook_xml` から読み取り済みの値をそのまま渡す）、[`resolve/shared_strings.rs`](../resolve/shared_strings.md)（本ファイルが定義する `PendingSharedString` を `use`）、[`resolve/style.rs`](../resolve/style.md)（同 `PendingStyle`）、[`resolve/mod.rs`](../resolve/mod.md)（`resolve_sheet` のシグネチャで両型を参照）、[`resolve/hyperlink.rs`](../resolve/hyperlink.md)（間接的——本ファイルの `PendingHyperlink` 自体ではなく、`pipeline.rs` がそこから構築する `HyperlinkRange` を受け取る）

**`PendingSharedString` / `PendingStyle` を本ファイルに定義する設計の経緯**: 当初案では両型を消費側（[`resolve/shared_strings.rs`](../resolve/shared_strings.md) / [`resolve/style.rs`](../resolve/style.md)）に定義し、本ファイルがそれを逆に `use` する構造だったが、これは「パーサー層（低レイヤー）→ 解決層（高レイヤー）」という不自然な逆方向依存を生む（循環はしないが、architecture.md 設計方針2が意図する「I/O層（`container`/`parse`）とドメインロジック（`resolve`）の分離」の精神に反する）。[PR #9 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204)を受け、両型は「フェーズ3の出力データそのもの」であるという実体に即して本ファイル（`parse/worksheet.rs`）へ定義を移設した。これにより依存の向きは次のとおり完全に一方向（DAG）となり、[`resolve/shared_strings.rs`](../resolve/shared_strings.md) が既に `parse::shared_strings::SharedStringTable` に依存していた（[resolve/mod.md 依存関係](../resolve/mod.md)）のと同じ「`resolve/` が `parse/` の構築済み構造化データに依存する」というパターンへ統一される:

```text
parse::worksheet ─┬─▶ resolve::shared_strings（PendingSharedStringをuse）
                   ├─▶ resolve::style（PendingStyleをuse）
                   └─▶ resolve::mod（resolve_sheetのシグネチャでPendingSharedString/PendingStyleを参照）
parse::shared_strings ─▶ resolve::shared_strings（SharedStringTableをuse）
```

`parse/` 配下のいずれのモジュールも `resolve/` 配下の型を `use` しない、というarchitecture.md設計方針2の精神に完全に合致する構造になった（旧オープンクエスチョン1を解決）。

## エラー処理方針

- XMLとして構文的に不正な場合は [`convert_xml_error`](mod.md) を通じて `Error::XmlParse` または `Error::ZipBombDetected` に変換する
- `<row>`/`<c>` の `r` 属性はいずれも省略可能で、省略時は直前の行/セルから暗黙的に位置を推論する（Issue #79。旧オープンクエスチョン4を解決）。`<row r="...">` の値が数値としてパースできない場合・`0` の場合・`MAX_ROW` を超える場合、および推論後の列が `MAX_COL` を超える場合は `Error::InvalidCellRef` を返す（`CellRef::from_a1` の上限チェックと同じ方針。行の外(`<row>` を一度も見ていない状態)で `<c>` の `r` が省略された場合も同様）。`<row>` の `r` が数値としてパースできない場合を `<col>`/`sheetFormatPr` と同じ `Error::InvalidPackage` ではなく `Error::InvalidCellRef` とするのは、`r` がここでも座標(セル参照と同種の情報)であり、`<c r="...">` の不正値と同じ扱いに揃えるため(PR #81レビュー)。エラーメッセージには元の属性値の文字列表現と `path` を含める
- `r` 属性の値が不正なA1形式（`CellRef::from_a1` が `Err` を返す）の場合はそのまま `Error::InvalidCellRef` を伝播する
- `<v>` の数値テキストが `f64` としてパースできない場合は `Error::InvalidPackage`（暫定。より専用のバリアントを設けるかは [error.md](../error.md) 側の見直しに委ねる）とする
- `t="d"` セルの `<v>` テキストが日付のみ・日付+時刻・時刻のみのいずれの形にも一致しない、または各数値要素の範囲が不正（月13、時24等）な場合は `Error::InvalidPackage`（上記の数値 `<v>` と同じ暫定方針。Issue #58）。小数秒・末尾のUTC/オフセット指定子・秒の省略はエラーにせず許容する（PR #80レビュー指摘1。`parse_iso8601_datetime` のdocコメント参照）
- `<mergeCell ref="...">` の `ref` 属性値が `"A1:C3"` の形式（`:` 区切りの2座標）でない場合、または各座標が不正なA1形式の場合は `Error::InvalidCellRef` を伝播する。結合範囲としての妥当性（開始・終了の大小関係、他範囲との重複）そのものの検証は行わず、そのまま `merge_regions` へ積んで [`resolve/merge.rs`](../resolve/merge.md) の検証に委ねる
- `<hyperlink ref="...">` の値が単一の正しいA1座標でも `"A1:C3"` 形式の2座標範囲でもない場合は `Error::InvalidCellRef` を伝播する（Issue #95）。範囲としての妥当性（開始・終了の大小関係、他のハイパーリンク範囲との重複）そのものの検証は行わない——`<mergeCells>` と同じ分担で [`resolve/hyperlink.rs`](../resolve/hyperlink.md) に委ねる
- `<col>` の `width`/`defaultColWidth` が `f64` として、または `min`/`max` が `u32` としてパースできない場合は `Error::InvalidPackage`（上記の数値 `<v>` と同じ暫定方針）を返す。範囲としての妥当性（重複・件数）そのものの検証は行わず、[`resolve/column_width.rs`](../resolve/column_width.md) に委ねる(`<mergeCells>` と同じ分担)
- `<f>`（数式）要素の内容はパース・保存せず読み飛ばす（要求仕様書のスコープ外。オープンクエスチョン2参照）
- 実際に `Sheet::insert_cell` されたセル数（`max_cells`引数、`SizeLimits::max_cells_per_sheet`。[container/sanitize.md](../container/sanitize.md)参照）が `max_cells` を超えた場合、`Error::TooManyCells` を返す（Issue [#88](https://github.com/MinamiyamaKotaro/xlsxparser/issues/88)）。値・スタイル・共有文字列参照のいずれも持たず握りつぶされたセルはカウントしない。バッチ収集後にまとめてチェックする `resolve::merge`/`resolve::column_width` とは異なり、`<c>` をストリーミングする最中に逐次カウント・チェックする——セルの場合、メモリコストは挿入された瞬間に発生するため、収集し終えてからのチェックでは手遅れになる
- **`panic` しない**: 本ファイルが扱う入力は信頼できない外部ファイルであるため、想定外の構造は必ず `Error` のいずれかのバリアントとして伝播させる

## テスト方針

- 単一行・複数セル（数値・共有文字列参照・真偽値・エラー値の混在）を持つ最小の `worksheet.xml` から、`Sheet` へ正しくセルが挿入されることの確認
- `t="s"` セルを検出した場合に `value: None` の `Cell` が挿入され、かつ対応する `PendingSharedString`（`cell_ref`/`index` が正しい）が記録されることの確認（不変条件の結線テスト）
- `s` 属性を持つセルについて、対応する `PendingStyle`（`cell_ref`/`style_id` が正しい）が記録されることの確認
- `t="str"` セルが `PendingSharedString` を経由せず、ストリーム中に直接 `CellValue::Text` として解決されることの確認
- `t="inlineStr"` セル（単純な `<is><t>...</t></is>` およびリッチテキストラン形式の両方）が正しく `CellValue::Text` として解決されることの確認
- 値も書式もない空のセル要素、または `<row>` に `<c>` が1つもない行が、`Sheet` に何も挿入しない（疎行列の空白セル非インスタンス化。要求仕様書3.1）ことの確認
- `<row>` の処理完了後、次の `<row>` の処理に影響を与えるパーサー内部状態が残らないことの確認（行単位破棄の回帰テスト。ある行で `t="s"` セルを処理した直後、次の行の通常セルが誤って共有文字列として扱われない、といったクロスコンタミネーション検出）
- `<mergeCells>` 内の複数 `<mergeCell ref="...">` から、`start`/`end` が正しい `MergedRegion` のリストが得られることの確認（妥当性検証自体は行わないため、開始・終了が逆転した不正な範囲もそのまま `Vec` へ含まれることの確認を含む。検証は [resolve/merge.md テスト方針](../resolve/merge.md) 側の責務）
- `r` 属性を欠く `<c>` が、同じ行内の直前セルの次列として正しく位置推論されることの確認。明示的な `r` を持つセルの直後に省略セルが続く場合、行頭からではなくその明示位置の次列から数え直されることの確認（Issue #79）
- `r` 属性を欠く `<row>` が、直前行の次番号として正しく位置推論されることの確認（Issue #79）
- `<row>` を一度も見ていない状態で `r` を欠く `<c>` が現れた場合、`<row r="...">` が数値としてパースできない場合、`<row r="0">` の場合、`<row>` の `r` が `CellRef::MAX_ROW` を超える場合、および省略推論後の列が `CellRef::MAX_COL` を超える場合(セキュリティレビュー Finding 2 と同じ攻撃面の回帰テスト)に、それぞれ `Error::InvalidCellRef` を返すことの確認（Issue #79。数値パース失敗のケースはPR #81レビューを受けて追加——`Error::InvalidPackage` になっていた旧挙動との回帰テスト）
- 不正なA1形式の `r` 属性・`mergeCell ref` 属性に対し `Error::InvalidCellRef` を返すことの確認
- `<hyperlinks>` 内の `<hyperlink ref="A1" r:id=".." location=".." tooltip=".."/>` から、正しい `start`/`end`/`r_id`/`location`/`tooltip` を持つ `PendingHyperlink` が得られることの確認——単一座標の `ref`（`start == end`）と範囲 `ref` の両方について。不正な `ref` は何も登録せず `Error::InvalidCellRef` を返すことの確認（Issue #95）
- `width` 属性を持つ `<cols>` の各エントリが、正しい `min`/`max`/`width` の `ColWidthRange` として収集されることの確認。`width` を持たない `<col>` はスキップされることの確認。単一の `<col min="1" max="16384" .../>`（実データの最悪ケース）が展開されず1件として収集されることの確認
- `<sheetFormatPr defaultColWidth="..">` が収集されること、および欠落時に `default_col_width: None` のままであることの確認
- `<col>`/`<sheetFormatPr>` の不正な数値属性に対し `Error::InvalidPackage` を返すことの確認
- `<f>` 要素を含むセル（数式セル）について、`<f>` の内容が無視され `<v>`（計算済みキャッシュ値）のみが `Cell` の値として採用されることの確認
- `t="d"` セルについて、日付のみ・日付+時刻・時刻のみの3パターンそれぞれが正しい `CellValue::DateTime` として解決されることの確認（時刻のみの場合、日付部分がExcelの規約どおり1899-12-30になることを含む）。不正な形式（範囲外の数値、セグメント数不一致）に対し `Error::InvalidPackage` を返すことの確認(Issue #58。実例として `tests/fixtures/other/date_iso.xlsx` ——calamineテストコーパス由来——で3パターンとも確認済みだが、同ディレクトリは`.gitignore`対象のため、統合テスト自体は同じ3パターンを再現した手書きフィクスチャを用いる)
- 末尾の `Z`/`+09:00` 等のUTC・オフセット指定子が読み捨てられること、小数秒(`10:10:10.500`)が整数秒へ切り捨てられること、秒省略(`10:10`)が0扱いになることの確認（PR #80レビュー指摘1）
- `date1904 = true` のブックで時刻のみの `t="d"` セルを解決した場合、仮の日付が1899-12-30ではなく1904-01-01になることの確認（PR #80レビュー指摘2。`resolve/style.rs` の数値ベース時刻のみセルとの整合性の回帰テスト）
- 実際に挿入されたセル数が `max_cells` を超えた時点で `Error::TooManyCells` が返り、それ以上パースを継続しないことの確認(Issue #88)。値・スタイル・共有文字列参照のいずれも持たない `<c>` は挿入されないため、いくら並べても `max_cells` にはカウントされないことの確認を含む(`tests/security.rs` 側で小さい `max_cells_per_sheet` を使い高速に検証。`tests/fixtures/security.rs` の `too_many_cells` フィクスチャ)

## 実装メモ

- **状態機械の形**: `<c>` の子要素（`<v>`/`<f>`/`<is>`）は互いに入れ子にならないため、専用の状態enumではなくフラットな `cur_*` ローカル変数（`cur_ref: Option<CellRef>` が「`<c>` の中にいるか」を兼ねる。加えて `cur_type`/`cur_style`/`cur_value_text`/`cur_inline`）で実装した。各変数は `<c>` の開始タグを見た瞬間に必ず新規初期化され、終了タグ（自己終了 `<c/>` の場合は即座に）までに共通ヘルパー `flush_cell` で消費し切る。行やセルを跨いで状態が漏れないことを実際に保証しているのは、明示的な行境界でのリセットではなく、この「セルごとの毎回フル初期化」である。
- **`build_cell` のシグネチャ**: ドラフトが未使用のまま `let _ = (cell_ref, style_id);` としていた `cell_ref`/`style_id` 引数は削除した。値/スタイルの振り分けは呼び出し元（`flush_cell`）が担い、`build_cell` 自体は不要。
- **`<v>`/`<f>` のテキスト読み取り**: ドラフトには無かった `read_leaf_text` ヘルパーを追加した。quick-xml 0.41 では実体参照が `Event::Text` とは別の `Event::GeneralRef` としてトークン化されるため（[parse/mod.md オープンクエスチョン1](mod.md)参照）、`concat_rich_text` と同じ共通ヘルパー `push_general_ref`（[parse/mod.rs](mod.md)）経由で解決する。
- **`flush_cell` の挿入判定**: `<c>` は、スタイル（`s`属性）・値（`<v>`/`<is>` のテキスト）・`t="s"`参照（解決後に値を持つ）のいずれかを持つ場合にのみ挿入する。疎行列の要件通り、完全に空の `<c r="A1"/>` はインスタンス化しない。
- **`flush_cell` の戻り値**（Issue #88）: 実際に挿入したかどうかを `bool` として返すよう変更した（元は `Result<(), Error>`、現在は `Result<bool, Error>`）。呼び出し元（`parse_worksheet`）はこれを見て、実際に挿入された場合だけセル数カウンタをインクリメント・上限チェックする——空セルはメモリコストがゼロなのでカウントに含めると正当なスパースファイルを不当に制限してしまうため。カウント・上限チェック自体は `check_cell_count` という小さな別関数に切り出し、`flush_cell` の2つの呼び出し箇所（自己終了 `<c/>` の場合と `Event::End` の場合)から共通で呼ぶ。

## 未決事項 / オープンクエスチョン

1. ~~`PendingSharedString` / `PendingStyle` の配置場所の再検討~~ → **解決**: 両型の定義を本ファイル（`parse/worksheet.rs`）へ移設し、[`resolve/shared_strings.rs`](../resolve/shared_strings.md) / [`resolve/style.rs`](../resolve/style.md) 側がそれぞれを `use` する構造とした（[PR #9 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204)を反映）。詳細は依存関係セクション参照。
2. **数式（`<f>` 要素）の扱い**: 現状は内容を一切パース・保存せず読み飛ばす方針（`<v>` の計算済みキャッシュ値のみを採用）を仮定している。数式文字列そのものをJSON出力に含める要求が将来生じた場合、`Cell` に `formula: Option<String>` を追加するか等は要求仕様書の詳細化と合わせて確定させる。
3. **未知の `t` 属性値へのフォールバック方針**: 現状は生の `<v>` テキストをそのまま `CellValue::Text` として保持するフォールバックとしているが、[parse/workbook.md](workbook.md) の `state` 属性フォールバックと同様の考え方（データを失わない側へ倒す）である一方、明確な `Error` とすべきという意見もありうる。
4. ~~`r` 属性省略セルの列位置逐次推論~~ → **解決**（Issue [#79](https://github.com/MinamiyamaKotaro/xlsxparser/issues/79)）: `<row>`/`<c>` いずれも `r` 省略時は直前の行/セルから位置を推論するようになった。ループのローカル状態（現在の行番号・行内の現在列）として `cur_row`/`cur_col` を追加し、`<row>` の開始タグで行番号確定・列位置リセット、各 `<c>` の開始タグで `r` があればそれを採用しつつ現在列を更新（後続の省略セルはその位置から数え直す）、無ければ現在列を1進めて採用する形で実装した。`CellRef::MAX_ROW`/`MAX_COL` の上限チェックは推論経路でも同様に行う（セキュリティレビュー Finding 2 と同じ攻撃面——`r` を省略した `<c>` を大量に並べるだけで `CellRef::from_a1` の上限チェックを経由せず `Sheet::max_col` を膨張させられてしまうため）。実例として確認していた `tests/fixtures/other/minimal_package.xlsx`（calamineテストコーパス由来、非コミット）はこの修正により実際にend-to-endで解決できることを確認済み。
5. **`Reader` の内部バッファサイズ・パフォーマンスチューニング**: [parse/mod.md オープンクエスチョン5](mod.md) と同一の論点。要求仕様書が想定する「方眼紙Excel」規模のシートに対する実測プロファイリングを踏まえて確定させる。
6. ~~名前空間の扱い~~ → **解決**: [parse/mod.md オープンクエスチョン4](mod.md) で確定した「`quick_xml::NsReader` は採用せず文字列前方一致で簡略化する」方針に従う。`worksheet.xml` 自体の要素・属性（`row`, `c`, `v`, `is`, `t`, `s`, `r`, `mergeCells`, `mergeCell`, `ref`）に接頭辞は付かないため、本ファイルへの直接的な影響はない。
7. ~~セル数の上限チェックをどこで・どう行うか~~ → **解決**（Issue [#88](https://github.com/MinamiyamaKotaro/xlsxparser/issues/88)）: `resolve::merge`/`resolve::column_width` のような「バッチ収集後に一括チェック」ではなく、本関数が `<c>` をストリーミングする最中に逐次カウント・チェックする方式を採用した。理由: セルの場合、メモリコストは `Sheet::insert_cell` された瞬間に発生するため、全セルを収集し終えてからチェックしたのでは既に手遅れになる。上限値自体は `SizeLimits::max_cells_per_sheet`（[container/sanitize.md](../container/sanitize.md)）として呼び出し側が設定可能にし、`pipeline.rs` が `parse_worksheet` の新規引数 `max_cells` として橋渡しする。
