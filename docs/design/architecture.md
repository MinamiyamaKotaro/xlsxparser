# src/ アーキテクチャ設計

*[English](architecture.en.md)*

Issue [#1](https://github.com/MinamiyamaKotaro/xlsxparser/issues/1) での議論を経て確定した `src/` ディレクトリ構成と、各モジュールの責務をまとめたドキュメント。要求仕様書（[requirements.md](../requirement/requirements.md)）が定義する5フェーズ・パイプラインに対応させている。

## 設計方針

1. **フェーズごとの責務分離**: rels解決 → サニタイズ → ストリームパース → 分析/遅延解決 → JSON生成という一方向パイプラインの各フェーズを、対応するモジュールに一対一で割り当てる。
2. **I/O とドメインロジックの分離**: ZIP展開・XMLパースといった I/O 層（`container/` `parse/`）と、共有文字列解決・結合セル解決・スタイル適用といったドメインロジック（`resolve/`）を分離する。`resolve/` 配下は I/O やXML構造に一切依存せず、`model::Sheet` などメモリ上のデータ構造のみで完結させることでテスト容易性を確保する。
3. **オーケストレーションの一元化**: `container` と `parse` は実際には「ZIPからバイト列を取得 → パース → 結果に基づき次のZIPエントリを取得」という形で密に往復する。この呼び出し順序とリソース（`ZipContainer` 等）のライフサイクル管理は `pipeline.rs` に一元化し、他のモジュールが互いを直接知らなくてよいようにする。
4. **命名規則**: `package` は Cargo package と混同しやすいため使用せず、OPC (Open Packaging Conventions) の性質を表す `container` を用いる。

## ディレクトリ構成

```text
src/
  lib.rs                  # 公開APIのエントリポイント (例: parse_workbook(path) -> Result<Workbook>)
  error.rs                # ライブラリ全体の共通エラー定義
  pipeline.rs              # フェーズ1〜5全体のオーケストレーター（I/Oとライフサイクル管理）

  container/               # I/O & セキュリティガード
    mod.rs                # ZIP展開のエントリポイント、安全なファイル取得
    sanitize.rs           # フェーズ2: Zip Bomb / Zip Slip 検知ロジック

  parse/                    # XMLパース専用（quick-xml依存コードを集約）
    mod.rs                # XMLパーサーの共通ヘルパー、フェーズ2: XXE無効化設定（Reader初期化）
    relationships.rs      # フェーズ1: _rels 解析（ルーティングマップ構築用データのパース）
    workbook.rs           # workbook.xml のパース
    shared_strings.rs     # sharedStrings.xml のパース（SSTの構造化データ抽出）
    styles.rs             # styles.xml のパース（fonts/fills/borders/numFmts/cellXfs）
    theme.rs              # theme{N}.xml の <clrScheme> のパース（Issue #76。スタイルがテーマ色を参照する場合のみ読む）
    worksheet.rs          # フェーズ3: sheetX.xml の SAXストリームパース（行単位の破棄はここで完結）。<mergeCells>/<cols>/<hyperlinks>(Issue #95)もここでフェーズ4向けに収集する
    drawing.rs             # フェーズ3.5: drawingN.xml の画像アンカー解析（Issue #65）

  model/                    # 純粋なドメインモデル（XMLパースや解決ロジックに依存しない）
    mod.rs
    cell.rs               # CellValue, Cell, CellRef (A1形式 <-> 座標)
    sheet.rs              # 疎行列 Sheet (BTreeMap<CellRef, Cell>)、結合範囲・列幅・画像・ハイパーリンクを保持
    workbook.rs           # 解決済みの Workbook モデル
    style.rs              # ResolvedStyle / StyleSheet / StyleId / Borders（parse/styles.rs と resolve/style.rs の共有語彙。PR #8 レビュー指摘を反映し新設）
    color.rs              # Rgb / ThemePalette——Issue #76の resolve_color が返す解決済み色の型

  resolve/                  # フェーズ4: 分析と遅延解決（I/O非依存、model::Sheet のみで動作）
    mod.rs                # フェーズ4の解決処理のエントリポイント
    shared_strings.rs     # 共有文字列(SST)のインデックス解決
    merge.rs              # 結合セルの遅延解決・エイリアス参照マッピング
    style.rs              # セルスタイルの適用
    column_width.rs       # <cols>列幅範囲の遅延解決（Issue #39）
    hyperlink.rs          # <hyperlink>範囲の重複検証+スイープラインによる解決（Issue #95）
    color.rs              # ColorRef -> Rgb のオンデマンド解決（Issue #76。セル単位パイプラインには含まれない）

  json.rs                   # フェーズ5: row_span/col_span を含むJSONシリアライズ
```

## モジュール責務の詳細

### `lib.rs`

クレートのルート。公開APIのエントリポイント（`parse_workbook(path) -> Result<Workbook>` 等）を定義するとともに、`container/` `parse/` `resolve/` `pipeline.rs` `json.rs` を非公開の `mod` として宣言してクレート内部実装として隠蔽し、`model/` の一部の型と `error::Error` のみを外部へ再エクスポートする。

- 詳細設計: [lib.md](lib.md)

### `error.rs`

クレート全体で共有する単一のエラー列挙型 `Error` を定義する。`model/` を含むクレート内の他モジュールに依存しない最も基底のリーフモジュールであり、`container/` `parse/` `model/` `resolve/` `pipeline.rs` `lib.rs` のほぼ全モジュールがこの型に依存する。

- 詳細設計: [error.md](error.md)

### `pipeline.rs`

`ZipContainer` を所有し、各フェーズの実行順序（`container` からストリームを借りて `parse` へ渡し、結果を `resolve` で解決して `json.rs` でシリアライズする）とリソース破棄タイミングを制御する。

- フェーズ1完了時（ルーティングマップ構築後）に `_rels` の一時バッファを破棄する。
- フェーズ4完了時（共有文字列・スタイルの解決完了後）に `SharedStringTable` や `StyleSheet` を破棄する。
  ※この破棄が成立するのは、`model::Cell` がインデックスではなく解決済みの実データ（`String` や `ResolvedStyle` の値、または `Arc` などの所有権付き参照）を直接保持する設計を取る場合に限る。セル側がインデックス／参照のみを保持する設計にする場合は、フェーズ5（JSON生成）が完了するまで `SharedStringTable` や `StyleSheet` の生存期間を維持する必要がある。
- 行単位のXMLノード破棄（フェーズ3）は `parse/worksheet.rs` 内部の実装詳細であり、`pipeline.rs` はこれを制御しない。ファイル/データ構造単位の破棄のみを担う。

- 詳細設計（Issue [#3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) で進行中のモジュール別設計書）: [pipeline.md](pipeline.md)

### `container/`

ZIP(OPC)展開のエントリポイント。Zip Bomb・Zip Slip の検知・ブロックを担う。XMLの中身の解釈（パース）は行わない。

- 詳細設計（Issue [#3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) で進行中のモジュール別設計書）: [mod.md](container/mod.md) / [sanitize.md](container/sanitize.md)

### `parse/`

`quick-xml` などXMLパースライブラリへの依存を集約する層。XML要素から純粋な構造体への詰め替えのみを行い、ビジネスロジック（結合セル解決・共有文字列解決など）は持たない。XMLパース時の外部エンティティ展開無効化（XXE対策）は quick-xml の `Reader` 初期化設定であるため、quick-xml依存を集約する本層（`parse/mod.rs`）の責務とする。

- 各パーサー（`workbook.rs` / `worksheet.rs` 等）が個別に `Reader` を初期化すると設定漏れのリスクがあるため、`parse/mod.rs` にセキュアな `Reader` 生成専用のファクトリ関数（例: `create_secure_reader`）を設け、XXE対策の一元適用を強制する。`parse/` 配下の各モジュールはこのファクトリ経由でのみ `Reader` を取得する。

`parse/worksheet.rs` は行/セルデータと `<mergeCells>`/`<cols>`/`<hyperlinks>` 情報をストリームで順次送出する。`parse/theme.rs` は「使う時だけ読む」方針で、`parse/styles.rs` が実際にテーマ色を参照するスタイルを解決した場合のみ読み込む（Issue #76）。

- 詳細設計（Issue [#3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) で進行中のモジュール別設計書）: [mod.md](parse/mod.md) / [relationships.md](parse/relationships.md) / [workbook.md](parse/workbook.md) / [shared_strings.md](parse/shared_strings.md) / [styles.md](parse/styles.md) / [theme.md](parse/theme.md) / [worksheet.md](parse/worksheet.md) / [drawing.md](parse/drawing.md)

### `model/`

`Cell` / `Sheet` / `Workbook` などの純粋なRustデータ構造を定義する。XMLパースや解決ロジックへの依存を持たない。疎行列（`BTreeMap<(row, col), Cell>`）によりメモリを最適化する。`BTreeMap`を採用している理由（`HashMap`からの変更）は[model/sheet.md](model/sheet.md)参照。

この疎行列という選択は、[README.mdのBenchmarks節](../../README.md#benchmarks)で密な`Vec`を使う読み取りライブラリ(`calamine`)と直接比較・計測されている——対象は`tests/fixtures/complex/extreme_sparse.xlsx`(Excelの実際の対角にある2セルのみ populated。境界矩形サイズの確保を試みると171億8千万要素になる)。`exceldiff`は正確に2件のマップエントリで済み数ミリ秒で完了するのに対し、密な配列を使う読み取りライブラリは数GBまでメモリを膨張させた末にOSにkillされる。READMEのリソース使用量の時系列プロット(`docs/benchmarks/extreme_sparse_memory.svg`)がこれを理論上の話ではなく具体的に示している。

- 詳細設計（Issue [#3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) で進行中のモジュール別設計書）: [mod.md](model/mod.md) / [cell.md](model/cell.md) / [sheet.md](model/sheet.md) / [workbook.md](model/workbook.md) / [style.md](model/style.md) / [color.md](model/color.md)

### `resolve/`

フェーズ4の分析・遅延解決を担う。I/OやXML構造に依存しないため、`model::Sheet` などメモリ上のデータのみを用いてユニットテストできる。

- `shared_strings.rs`: `t="s"` のインデックスを `SharedStringTable` の実文字列に解決する。
- `merge.rs`: ストリーム完了後に `<mergeCells>` の結合範囲リストとセルデータを突き合わせ、仮想セル座標から起点セルへのエイリアス参照をマッピングする。
- `style.rs`: `styles.xml` から解決済みの書式情報をセルに適用する。
- `column_width.rs`: `<cols>` の列幅範囲を検証(開始・終了の大小関係、重複、件数上限)し `Sheet` へ登録する(Issue #39)。
- `hyperlink.rs`: `<hyperlink>` 範囲を`resolve::merge`と同型の重複検証にかけたうえで、既にセル化済みの全カバーセルへ1回のスイープラインパスで解決する(Issue #95)——`Sheet::finalize_merges`が結合セルについて解消したのと同じ理由で、セル単位の走査は行わない。
- `color.rs`: 上記のセル単位パイプラインには含まれない——`resolve_color`は`ColorRef`から`Rgb`への変換を呼び出し元が実際に必要な時だけ呼ぶ、純粋なオンデマンド関数(Issue #76)。

- 詳細設計（Issue [#3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) で進行中のモジュール別設計書）: [mod.md](resolve/mod.md) / [shared_strings.md](resolve/shared_strings.md) / [merge.md](resolve/merge.md) / [style.md](resolve/style.md) / [column_width.md](resolve/column_width.md) / [hyperlink.md](resolve/hyperlink.md) / [color.md](resolve/color.md)

### `json.rs`

分析・解決が完了したデータモデルを、`row_span` / `col_span` などフロントエンド描画に必要な属性を含むJSONへシリアライズする。

- 詳細設計（Issue [#3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) で進行中のモジュール別設計書）: [json.md](json.md)

## 議論の経緯

構成の妥当性検証および段階的な改訂の詳細は Issue [#1](https://github.com/MinamiyamaKotaro/xlsxparser/issues/1) のコメント履歴を参照。主な論点は以下の通り:

- `package/` → `container/` への改名（Cargo package との命名衝突回避）
- XMLパースコードの `parse/` への集約（技術スタックの隠蔽、テスト容易性の向上）
- 共有文字列解決の置き場所の明確化（`resolve/shared_strings.rs`）
- `container` と `parse` の往復呼び出しに対するオーケストレーション層（`pipeline.rs`）の新設
- 行単位破棄（`parse` 層の内部詳細）とファイル単位破棄（`pipeline.rs` が制御）の粒度分離
- XXE無効化設定の置き場所を `container/sanitize.rs` から `parse/mod.rs` へ変更（ZIP層の脅威であるZip Bomb/Zip SlipとXMLパーサー設定であるXXE対策は別レイヤーの関心事であり、後者は「quick-xml依存を`parse/`に集約する」という設計方針（設計方針2）と一致させるべきという指摘のため）
