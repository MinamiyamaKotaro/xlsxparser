# `src/` セキュリティコードレビュー

*[English](code-review.en.md)*

`docs/security/design-review.md` は実装開始前の**設計書**を対象にしたレビューだった。本レビューは、PR #30（Issue #28 テストフィクスチャ整備）マージ後の `master`（2026-08-17時点）における **`src/` 配下の実装コード**を対象に、静的解析（SAST）視点で改めて実施したものである。OWASP Top 10 の観点に加え、Rust特有の観点（`unsafe`・パニック安全性・整数オーバーフロー・アルゴリズム的計算量）でスキャンし、実際にコードを動かして再現・実測した上で報告する。

## 総評

Zip Bomb・Zip Slip・XXE という要求仕様書が明示する3脅威への対策は実装レベルでも健全に一貫しており、`unsafe` コードは1箇所も存在せず、`cargo audit` でも既知脆弱性は検出されなかった（2026-08-17時点、30クレート）。一方で、**「入力バイト数さえ上限を設ければDoSは防げる」という設計時の前提が誤りであったこと**を示す実装済みコードのバグを1件（Finding 1）実測込みで発見した。これは `docs/security/design-review.md` および `docs/design/resolve/merge.md` が明示的にスコープ外・許容リスクとした箇所であり、その判断の前提自体を覆す内容だったため最優先で対応し、レビュー直後に修正・実測での再検証・回帰テストの追加まで完了させた（詳細はFinding 1参照）。加えて、座標値の妥当性検証が不足していた箇所（Finding 2）も同様に修正済み。サードパーティ製XMLパーサーの既定設定への暗黙の依存（Finding 3, 情報提供レベル）は未対応のまま残っている。

## Findings

### ~~Finding 1: `<mergeCell>` の重複検証がO(N²)であり、Zip Bomb対策のバイト数上限では防げない規模のCPU枯渇DoSを引き起こせる~~ → **解決**

* **脆弱性の種類**: アルゴリズム的計算量の悪用によるサービス拒否（CWE-407 Algorithmic Complexity / OWASP API4:2023 Unrestricted Resource Consumption 相当）
* **リスクレベル**: ~~High~~ → 解決済み
* **対象**: [`src/resolve/merge.rs`](../../../src/resolve/merge.rs) `resolve()` / `validate_region()`
* **内容（指摘当時）**:
  `resolve()` は `<mergeCells>` から集めた `N` 件の `MergedRegion` を1件ずつ検証しながら `accepted: Vec<MergedRegion>` に積み上げていくが、`validate_region` は新規の1件を検証するたびに **`accepted` に積まれた全件と重複判定を行う**（`for other in accepted { if regions_overlap(region, other) ... }`）。1回あたりの重複判定自体は矩形の幾何交差判定でO(1)だが、これをN件×(それまでの累積件数)回行うため、シート全体ではO(N²)になる。

  この設計判断自体は把握・記録済みで、[`docs/design/resolve/merge.md`](../../design/resolve/merge.md)（「範囲の件数Nが非常に多い場合はO(N log N)へ改善する余地があるが、実務上万単位に達するケースは稀なのでO(N²)で十分」）および [`docs/security/design-review.md`](design-review.md)（「結合範囲の検証アルゴリズムの計算量（O(N²)）…は、Zip Bomb対策でバイト数自体が有界であることを踏まえ、本レビューのスコープ外とした」）の双方が、**「Zip Bombのバイト数上限（既定512MiB/エントリ）が既にNを実質的に有界にしている」という前提のもとリスクを許容している**。

  しかし実測すると、この前提は成立しない。`<mergeCell ref="A{i}:B{i}"/>` は1件あたり約20〜30バイトしかなく、512MiBの上限内には理論上1,700万件以上収まる。実際に手元で計測したところ、以下のように綺麗な二乗則で増大した（Apple M2 Pro, release build）:

  | N (件数) | 実測時間 | 圧縮後ファイルサイズ |
  |---:|---:|---:|
  | 5,000 | 8.4 ms | 22 KB |
  | 10,000 | 29.2 ms | 42 KB |
  | 20,000 | 110.0 ms | 81 KB |
  | 40,000 | 424.3 ms | 158 KB |

  この実測値から外挿すると、**約N=194,000件（圧縮後ファイルサイズはおよそ1MB未満）で処理時間は約10秒に達し**、N=1,000,000件（数MB程度、512MiBの上限には遠く及ばない）では**数分間の完全なCPU拘束**が発生する。N=1,700万件（バイト数上限ぎりぎり）まで攻撃者が近づけば、事実上無限に終わらないハングになりうる。

* **攻撃シナリオ（指摘当時）**: 攻撃者は、`<mergeCell>` を重ならないよう規則的に大量生成しただけの、数百KB〜数MBという一見ごく普通のサイズの `.xlsx` を、本ライブラリを使ったアップロード処理（帳票取込・データ連携等）に提出する。ファイルサイズやZip Bomb対策のチェックはいずれも通過するため、Zip Slip/XXEのような即時エラーにはならず、`parse_workbook` を呼び出したスレッド（多くの場合Webサーバーのリクエストハンドラ）が数十秒〜数分間ブロックされる。同時に複数リクエストを送れば、スレッドプール/ワーカーを容易に枯渇させられる。
* **対応内容**: 提案A（防御的上限）を採用し実装した。[`src/resolve/merge.rs`](../../../src/resolve/merge.rs) に `pub(crate) const MAX_MERGE_REGIONS: usize = 20_000` を追加し、`resolve()` の冒頭で `regions.len() > MAX_MERGE_REGIONS` を検証、超過時は新設の `Error::TooManyMergedRanges { count, limit }`（[`src/error.rs`](../../../src/error.rs)）を返すようにした。この判定はO(N²)の重複検証ループより**前**に行われるため、上限超過時は件数に依存せずO(N)（XMLをストリーミングで読み集めるコストのみ）で即座に返る。

  修正後に同じ手法で再実測し、対策が効いていることを確認した（Apple M2 Pro, release build）:

  | N (件数) | 修正前 | 修正後 |
  |---:|---:|---:|
  | 1,000,000 | 数分（外挿値） | 260 ms（`Err(TooManyMergedRanges)`） |
  | 5,000,000 | 実質ハング（外挿値） | 1.32 s（`Err(TooManyMergedRanges)`） |

  修正後の所要時間はXMLの字句解析コスト（O(N)、既にZip Bomb対策のバイト数上限が間接的にカバーする領域）にほぼ比例しており、O(N²)の重複検証ループはもはや実行されない。N=20,000（上限ちょうど）で全件検証まで通す回帰テストと、N=20,001でO(N²)ループに入る前に即座にエラーになることを確認する回帰テストを [`src/resolve/merge.rs`](../../../src/resolve/merge.rs) の `region_count_at_the_limit_is_accepted` / `region_count_over_the_limit_is_too_many_merged_ranges`、および実際のXMLストリームを通す統合レベルの回帰テストを [`src/pipeline.rs`](../../../src/pipeline.rs) の `excessive_merge_cell_count_is_too_many_merged_ranges` として追加した。

  根本対策（開始行/列でソートしてスイープライン法によりO(N log N)へ改善する案、`docs/design/resolve/merge.md` が将来の改善候補として既に言及）は見送った。上限20,000は実務上の結合セル件数（数十〜数百件が大半）に対して十分な余裕があり、防御的上限だけで実害を解消できるため。`SizeLimits` のような呼び出し側からの上書きオプション化は、必要になった時点で別途検討する。

### ~~Finding 2: `CellRef` の行・列番号がExcelの実仕様上限にクランプされておらず、`maxRow`/`maxCol` を経由して下流消費者への資源枯渇を助長しうる~~ → **解決**

* **脆弱性の種類**: 入力値検証の不備が下流に伝播する問題（CWE-1284寄り。本ライブラリ自体はクラッシュしないが、結果を信頼する呼び出し側に対する攻撃を成立させうる）
* **リスクレベル**: ~~Medium~~ → 解決済み
* **対象**: [`src/model/cell.rs`](../../../src/model/cell.rs) `CellRef::from_a1`
* **内容（指摘当時）**: `from_a1` は行番号が `u32` に収まり `0` でないことしか検証しておらず、Excelの実際の上限（行: 1,048,576、列: 16,384 = `XFD`）を超える座標も有効な `CellRef` として受理する。実際に `<c r="ZZZZZZ4294967294">` を含む数百バイトの `.xlsx` を作成して読み込ませたところ、次のJSONが得られた（実測、`xlsxparser` 自体は正常終了）:

  ```json
  {"sheets":[{"name":"Sheet1","visibility":"visible","maxRow":4294967294,"maxCol":321272406,"cells":[{"row":4294967294,"col":321272406,"value":{"type":"number","value":1.0}}]}]}
  ```

  `xlsxparser` 自身は座標をキーにした `HashMap` に1件保持するだけなので影響を受けない（README.md「Benchmarks」で示した設計上の利点そのもの）。しかし `maxRow`/`maxCol` は「シートの外接矩形」として下流に**そのまま**渡され、これを信頼して密な配列・グリッドを確保しようとするフロントエンドや別サービスが存在すれば、本レビュー中に `calamine` で実際に観測した（README.md「Benchmarks」参照）のと同種の割り当て試行によるOOM/プロセスkillを誘発できる。
* **攻撃シナリオ（指摘当時）**: 攻撃者が座標を偽装した `.xlsx` をアップロードする。`xlsxparser` は問題なく解析しJSONを返すが、そのJSONの `maxRow`/`maxCol` を信頼してスプレッドシートUIを描画しようとするフロントエンド（またはExcel再エクスポート機能等）が、実在しない巨大な行・列数に基づいて配列確保を試み、クラッシュまたはメモリ枯渇に至る。
* **対応内容**: 提案通りの修正を実装した。[`src/model/cell.rs`](../../../src/model/cell.rs) の `CellRef` に `pub const MAX_ROW: u32 = 1_048_576` / `pub const MAX_COL: u32 = 16_384` を追加し、`from_a1` の既存の `row == 0` チェックと同じ箇所で `row > Self::MAX_ROW || col > Self::MAX_COL` を検証、超過時は既存の `Error::InvalidCellRef` を返すようにした（新しいエラー種別は不要）。

  `"XFD1048576"`（境界値ちょうど、Excelの実際の最大セル）は従来通り成功することを既存の `from_a1_to_a1_round_trip` テストで確認済み。新規に `"A1048577"`（行が1超過）・`"XFE1"`（列が1超過）を `from_a1_rejects_invalid_strings` に追加し、`ZZZZZZ4294967294`（実測に使った座標そのもの）を直接検証する回帰テスト `from_a1_rejects_row_or_col_far_beyond_excels_real_maximum`、および実際のworksheet XMLを通す統合レベルの回帰テスト `cell_ref_beyond_excels_real_maximum_is_invalid_cell_ref`（[`src/pipeline.rs`](../../../src/pipeline.rs)）を追加した。

### Finding 3（情報提供レベル）: リッチテキスト読み取りの深さカウンタが、サードパーティXMLパーサーの既定設定に暗黙に依存している

* **脆弱性の種類**: 暗黙の前提への依存（`docs/security/design-review.md` Finding 1で一度指摘・解決されたのと同種のパターンが、別モジュールに再度存在する）
* **リスクレベル**: Low（現状のコードでは到達不能と判断。将来の変更に対する多層防御の提案）
* **対象**: [`src/parse/mod.rs`](../../../src/parse/mod.rs) `concat_rich_text` の `skip_depth: u32`
* **内容**: `concat_rich_text` は `<rPr>`/`<rPh>` の開始・終了タグで `skip_depth` を増減する（`u32`）。もし `</rPr>` が対応する `<rPr>` なしに現れた場合、`skip_depth -= 1` は `skip_depth == 0` の状態で呼ばれ、デバッグビルドではパニック、リリースビルドでは `u32::MAX` へのラップアラウンドが発生する。

  検証したところ、この経路は**現状のコードでは到達不能**である。`create_secure_reader` が構築する `quick_xml::Reader` は `check_end_names`（既定値 `true`、`quick-xml 0.41.0` の `Config::default()` で確認済み）を変更していないため、対応しない終了タグは `Event::End` として `concat_rich_text` に届く前に `read_event` の時点でXML構文エラーとして弾かれる。
* **リスクシナリオ**: 現状は攻撃不可能。ただし、この安全性は `concat_rich_text` のコード自体には一切記述されておらず、`quick-xml` のデフォルト設定という外部クレートの挙動のみに支えられている——`docs/security/design-review.md` Finding 1が別モジュール（XXE対策）で一度明示的に問題視し、`read_event` を明示的な安全装置として導入して解決したのと全く同じ構造のリスクである。将来 `check_end_names` を明示的に無効化する変更が入る、あるいは別のXMLパーサーへ移行する判断がなされた場合、この暗黙の前提だけが崩れ、DoSが復活しうる。
* **推奨対応**: 多層防御として `skip_depth -= 1` を `skip_depth = skip_depth.saturating_sub(1)` に変更する（コスト実質ゼロ）。加えて、この安全性が `check_end_names` の既定値に依存している旨をコードコメントとして明記する。

## 良好だった点

* `unsafe` コードは `src/` 全体で0件。
* `cargo audit`（2026-08-17時点、30クレート走査）で既知脆弱性は検出されなかった。
* Zip Bomb（`container/sanitize.rs::BoundedReader`、ZIPヘッダーの自己申告サイズを信頼せず実読み取りバイト数でカウント）・Zip Slip（`validate_entry_path`、全エントリ名を開封時と参照時の二重に検証）・XXE（`read_event`、DOCTYPE宣言の存在のみで無条件拒否）はいずれも実装レベルで設計書通り健全に機能しており、`tests/real_error.rs`/`tests/security.rs` 等で実データ・実攻撃ペイロードを用いた回帰テストも整備されている。
* Finding 1・2以外の箇所では、`u32`算術のオーバーフロー可能性を個別に検証したが、`CellRef` の行・列がいずれも `>= 1` である不変条件により `MergedRegion::row_span`/`col_span` の減算は理論上の最大値でもオーバーフローしないことを確認した。
* `.expect()` はプロダクションコードに2箇所（`resolve/style.rs`、`resolve/shared_strings.rs`）のみ存在するが、いずれも「`parse/worksheet.rs` がPending*記録とセル挿入を必ず同時に行う」という同一モジュール内で完結する不変条件に依拠しており、`Sheet::insert_cell`/`insert_merge` がセルを削除する経路を持たないことも確認した上で、悪意あるファイル内容からは到達不能と判断した。
