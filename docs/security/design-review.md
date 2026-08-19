# `docs/design/` セキュリティレビュー (2026-08-19)

`docs/design/`配下の全設計文書([architecture.md](../design/architecture.md)を起点に、そこからリンクされる`lib.rs`/`error.rs`/`pipeline.rs`/`container/`/`parse/`/`model/`/`resolve/`/`json.rs`の各文書を含む)を対象とした、`master`上の現在の状態に対するフォローアップのセキュリティレビュー。[`old/design-review.md`](old/design-review.md)(2026-08-17、実装着手前)および[`old/code-review.md`](old/code-review.md)(2026-08-17、初回のコードレベルレビュー)の後継にあたる。最初のレビュー時と異なり、現在`src/`は完全に実装されており、「設計→実装→ドキュメント更新」というプロセスを通じて設計文書自体も実装と同期され続けてきた。最初のレビューが見ていなかった複数の機能追加の波(画像アンカーとグループ化画像(Issue #65/#67)、`styles.xml`からのフォント/折返し/配置/書式コード/塗りつぶし色の解決(Issue #37/#38/#41/#42/#75))を含む。本レビューは2026-08-17以降に変わった点——新たに追加された読み取り経路における3大脅威の健全性の継続、および追加された内容に含まれる新しい設計レベルのリスクパターン(計算量、未検証の数値フィールド、未解決のセキュリティ関連オープンクエスチョン)——に焦点を当てる。

以下のfindingは、設計文書自身の記述が具体的で検証可能な主張("上限が存在しない"、"これはO(1)である"等)をしている箇所について`src/`と突き合わせて確認しており、1件は直接の実測(Finding 1)を伴う。本レビューはあくまで設計文書レビューであり、`src/`の全行を対象にしたコード監査ではない——並行するコードレベルのレビューが`src/`の行単位の精査を担う想定。

## 総合評価

要求仕様書が名指しする3つの脅威——Zip Bomb・Zip Slip・XXE——は、前回レビュー以降に追加された全モジュールにわたって、健全かつ*一貫して*緩和され続けている。`parse/drawing.rs`(drawing/グループ化画像のパース)と`parse/styles.rs`(styles.xmlのパース)はいずれも、他の`parse/`モジュールと全く同じ必須ゲートウェイ——`Reader`構築のための`create_secure_reader`、DOCTYPE拒否によるXXEゲートの`read_event`、Zip Bombを意識したエラー変換の`convert_xml_error`——を経由しており、`pipeline.rs`の新設フェーズ3.5(画像解決)も、他のすべてのOPCパーツと同じ`container::get_entry`/`parse::parse_relationships`ゲートウェイを通じて`drawingN.xml`/`drawingN.xml.rels`を読み込み、`Internal`な画像ターゲットの存在確認を`has_entry`経由で(バイト列を読む前に)再検証している。既存のゲートウェイを迂回する新しい読み取り経路は見つからなかった。本プロジェクト自身の過去のfinding(バイト数上限の裏に隠れた、攻撃者が制御可能な件数Nを実際には制限しないO(N²)コスト——`docs/security/old/code-review.md` Finding 1、結合セル)の教訓は明らかに内在化されている: `resolve/column_width.rs`(Issue #39、当該finding後に追加)は`MAX_MERGE_REGIONS`の論拠を明示的に引用し、自身のアルゴリズムにO(N²)リスクが無いにもかかわらず同等の防御的上限を適用しており、`parse/drawing.rs`自身の`chExt=0`除算ゼロガードも、本レビューが確認を求められていた数値安全性チェックそのものである。

しかし、同じ「バイト数上限が攻撃者制御可能なNを制限しない」というパターンが、Issue #67(グループ化画像)で追加された内容の中に、検出されないまま再発していた——今回は*件数*ではなく*深さ*として、二乗ではなく乗算として。これは発見・実測され——結合セルの当初のfindingと全く同じ前例に従い——本レビューサイクルの一部として直ちに修正された。修正と再計測の詳細はFinding 1を参照。副次的な、実害の無い(しかし実在する)ギャップ(Finding 2)として、`docs/design/`内のいくつかの「主要な型」コードブロックが、その散文/依存関係/テスト方針の各節が説明している機能の実装に追従して更新されていなかった点があり、これはまさにFinding 1が今回まで文書ベースのレビューで見過ごされる原因となったのと同種のギャップである。

## Findings

### ~~Finding 1: グループ化画像解決における`<xdr:grpSp>`ネスト深さの無制限が、既存のあらゆるサイズ/件数上限の内側でCPU枯渇DoSを可能にする~~ → **解決済み**

* **深刻度: ~~High~~ → 解決済み**
* **箇所**: [`parse/drawing.md`](../design/parse/drawing.md)「グループ化画像: `GroupContext`と`resolve_grouped_pic`(Issue #67)」節およびそのオープンクエスチョン4・5。実装は`src/parse/drawing.rs`(`parse_anchor_body`内の`group_stack: Vec<GroupContext>`、および`</xdr:pic>`ごとに1回呼ばれる`resolve_grouped_pic`)。

* **詳細**: `parse_anchor_body`は`group_stack: Vec<GroupContext>`を保持し、`<xdr:grpSp>`の開始ごとにpush、終了ごとにpopするが、**深さの上限が一切無い**——設計文書は「スタック自身の長さが常に現在のネスト深さである——別途カウンタは不要」とのみ述べており、上限が強制されているとは一切述べていない。`src/parse/drawing.rs`のどこにもそのような上限は存在しない(確認済み: `resolve/merge.rs`の`MAX_MERGE_REGIONS`や`resolve/column_width.rs`の`MAX_COLUMN_WIDTH_RANGES`——両者の設計文書は明示的に文書化・上限設定している——とは異なり、当該ファイルには`MAX_*`定数が一切登場しない)。

  `resolve_grouped_pic(group_stack, ..)`は、グループツリー内のどこであれ`</xdr:pic>`終了タグが見つかるたびに1回呼ばれ(`src/parse/drawing.rs`194行目付近)、そのコストは**O(現在のネスト深さ)**である——`group_stack`*全体*を逆順に走査し、各階層の線形変換(`docs/design/parse/drawing.md`自身の記述: `child' = off + (child - chOff) * (ext/chExt)`、「最内層から最外層へ」適用)を適用する。`<xdr:grpSp>`は任意の深さで任意個の兄弟`<xdr:pic>`要素を含みうり、DrawingMLにはネスト深さへのスキーマレベルの制限が無いため、1つのdrawingパーツが`D`段のネストと最内層の`N`枚の兄弟画像を持つ場合、総パースコストは**O(N × D)**になる——一方でそのようなファイルを構築するXMLバイトコストはO(N + D)(加算のみ)にすぎない。これはまさに、既に修正済みの結合セルのfinding(`docs/security/old/code-review.md` Finding 1: バイト数上限がNを制限しないことに隠れたO(N²)コスト)と同じ形——ただし今回は1つの件数の自乗ではなく、互いに独立して安価に膨張させられる2つの軸(深さと兄弟数)の掛け算であり、結合セルの場合と異なり、**どちらの軸についても防御的な上限が一度も追加されていなかった**。

* **実測による確認**: `parse_drawing`(設計文書が説明する純粋なXMLパースのエントリポイントそのもの)を直接呼び出す一時的な`#[ignore]`付きユニットテストを追加し、`<xdr:grpSp>`のネストと兄弟`<xdr:pic>`数を合成的に生成して計測後、元に戻した(作業ツリーに残留する変更なし——`src/parse/drawing.rs`のgit履歴はクリーン)。現在の`master`ビルド(releaseプロファイル、Apple系ハードウェア、シングルスレッド)での計測:

  | ネスト深さ(D) | 最内層の画像枚数(N) | XMLサイズ | 所要時間 |
  | ---: | ---: | ---: | ---: |
  | 100 | 100,000 | 29.8 MB | 191 ms |
  | 100,000 | 100 | 15.5 MB | 193 ms |
  | 5,000 | 5,000 | 2.26 MB | 135 ms |
  | 20,000 | 2,000 | 3.69 MB | 222 ms |
  | 50,000 | 1,000 | 8.0 MB | 325 ms |
  | 100,000 | 5,000 | 17.0 MB | **2.33 秒** |
  | 50,000 | 50,000 | 22.6 MB | **10.9 秒** |

  最初の2行(Nが大きくDが小さい、またはDが大きくNが小さい——すなわちどちらか一方の軸のみ)は安価なままであり、コストが実際に2軸の*積*であって、どちらか単独の値ではないことを裏付けている。最後の2行は、わずか17〜23MB——512MiBの1エントリあたりZip Bomb上限にも2GiBの累積上限(`container/sanitize.md`の`DEFAULT_MAX_UNCOMPRESSED_SIZE`/`DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE`)にも遠く及ばず、他のいかなる既存の指標から見ても異常に大きなワークブックとは言えないファイル——が、通常の`parse_workbook`/`parse_workbook_reader`実行中に`pipeline.rs`のフェーズ3.5から呼ばれる`parse_drawing`内で、既に数秒の完全に同期的なCPUブロッキングを発生させることを示している。結合セルのfindingで測定されたのと同じ二乗則の伸びで外挿すると、50〜100MB規模(1エントリあたり上限には依然遠く及ばない)のdrawingパーツは、修正前の結合セルfindingと同じ深刻度クラスである数十秒〜数分に達することが十分ありうる。

* **部分的な緩和要因**: `group_stack`は明示的にヒープ確保された`Vec`であり、ネイティブなコールスタック再帰ではない。そのため、素朴な再帰下降型のXMLネストパーサーとは異なり、深さ単体に起因するスタックオーバーフローのリスクは無い——コストはCPU時間(および副次的に`Vec<GroupContext>`自身のヒープ成長、CPUコストと比べれば無視できる)である。これは設計上の実際の長所であり「設計上妥当と判断した点」に記載するが、上記のCPU枯渇分析自体は変わらない。

* **リスクシナリオ**: 攻撃者が、深くネストした`<xdr:grpSp>`ツリーと最内層に中程度の枚数の兄弟画像を持つ`drawingN.xml`を含む`.xlsx`(または`parse_workbook`/`parse_workbook_reader`を未検証入力に対して呼ぶあらゆるシステム、例えば文書アップロード機能)を送信する——数十MB規模で、文書化・強制されているあらゆるサイズ/件数上限の内側に十分収まり、既存のあらゆるチェック(Zip Bomb・Zip Slip・XXE・`MAX_MERGE_REGIONS`・`MAX_COLUMN_WIDTH_RANGES`)のどれにも引っかからずに通過する。呼び出し元スレッド(典型的にはWebサーバーのリクエストハンドラ)は`parse_drawing`内で数秒〜場合によっては数分ブロックされる。この形状のリクエストを少数同時に送るだけで、スレッド/ワーカープールを枯渇させられる——結合セルの当初のfindingのリスクシナリオの組み立てと完全に一致する。

* **解決内容**: 推奨事項通りに実装した。`src/parse/drawing.rs`は現在`pub(crate) const MAX_GROUP_NESTING_DEPTH: usize = 64`を定義し、`parse_anchor_body`の`b"grpSp"`開始タグ分岐内でチェックする——ネストしたグループの開始タグが`group_stack`をこの上限より深く積もうとした場合、新設した`Error::TooManyNestedGroups { path, depth, limit }`([`src/error.rs`](../../src/error.rs)、`Error::TooManyMergedRanges`/`Error::TooManyColumnWidthRanges`と同じ形)を、そのグループのそれ以上の内容を読む前に即座に返す。

  同時に、関連する2つ目の上限も追加した: `resolve_grouped_pic`の最終的な解決後の`(x, y, cx, cy)`について、`is_finite()`および防御的な妥当性上限(`MAX_PLAUSIBLE_EMU = 10^12`、約27.7km——Excelの実際の最大シート範囲を大きく上回る)に対するチェックを行い、いずれかに失敗した場合は`Error::InvalidPackage`を返す——これにより、`MAX_GROUP_NESTING_DEPTH`に到達するよりずっと前に、少数のネスト段数で極端な`ext`/`chExt`比率によって解決後の座標を`f64::INFINITY`(Rustの飽和的なfloat→int変換により`i64::MAX`へ静かに飽和する)まで到達させられるという、関連するより小規模な問題を解消している——この2件目の問題の詳細は[`code-review.md`](code-review.md) Finding 2を参照。

  `src/parse/drawing.rs`に回帰テストを追加した: `group_nesting_depth_at_the_limit_is_accepted`/`group_nesting_depth_over_the_limit_is_too_many_nested_groups`が64/65のちょうど境界をカバーし(`resolve/merge.rs`の`region_count_at_the_limit_is_accepted`/`region_count_over_the_limit_is_too_many_merged_ranges`のパターンに倣う)、`extreme_group_transform_scale_is_rejected_as_invalid_package`が2件目の問題をカバーする。

  修正後に上表と同じ攻撃形状(以前は無制限に実行されていたD=100,000/N=100の形状)を再計測したところ、**1ミリ秒未満で拒否**された——O(N × D)の変換処理が一切走る前に、深さチェックがネスト段数65で発火するため。

  `docs/design/parse/drawing.md`のエラー処理方針・テスト方針・未決事項(新規項目7)を更新し、両方の上限を文書化した。

### Finding 2: いくつかの設計文書の「主要な型」コードブロックが、その散文が説明する機能の実装に追従して更新されておらず、文書のみに基づくセキュリティレビューが新規追加の数値フィールドを検証する能力を損なっている

* **深刻度: Low(文書・プロセス上のギャップであり、それ自体は脆弱性ではない)**
* **箇所**: [`model/sheet.md`](../design/model/sheet.md)(「主要な型」コードブロック、14〜240行目に`Image`/`ImageAnchor`/`AnchorMarker`/`ImageExtent`/`ColWidthRange`の定義が無い。すぐ下の「機能: 画像(Issue #65)」「機能: 列幅(Issue #39)」の散文節、および依存関係節は、これらについて詳細に論じているにもかかわらず); [`json.md`](../design/json.md)(「主要な型」コードブロックに`images`/`columns`のシリアライズコードが無い——`JsonImage`/`JsonImageAnchor`/`JsonAnchorMarker`型も`state.serialize_field("images", ..)`呼び出しも無い。自身の依存関係節は`Sheet::images`、`Image`、`ImageAnchor`、`AnchorMarker`を依存先として明示しており、テスト方針にも`images`のシリアライズ形式に関する専用項目があるにもかかわらず); [`model/mod.md`](../design/model/mod.md)(その再エクスポート一覧`pub use sheet::{MergedRegion, Sheet, SheetVisibility};`は、`ColWidthRange`・`Image`・`ImageAnchor`・`AnchorMarker`・`ImageExtent`を欠いている——`json.md`自身の依存関係の記述`crate::model::{ColWidthRange, Sheet}`(`resolve/column_width.md`で使用)によれば、これらは全て今日の`src/model/sheet.rs`に存在する)

* **詳細**: 本プロジェクトの明言された進め方(ユーザー自身の標準的な指示による)は設計→実装→テストであり、各機能が着地した後は`docs/design/`が能動的に同期され続ける——本レビュー全体を通じてこれはほぼ完璧に維持されていた(Issue #37/#38/#39/#40/#41/#42/#65/#67/#75のいずれも、正確な構造体・フィールド名、エラーバリアント、さらには実測されたベンチマーク数値まで含む、実装に忠実な詳細な散文を残していた)。しかし、`drawingN.xml`から読み取られ公開JSON出力へそのまま伝播する攻撃者制御可能な数値データを保持する`AnchorMarker { cell, col_off: i64, row_off: i64 }`/`ImageExtent { cx: i64, cy: i64 }`/`ImageAnchor`/`Image`という実際のRust構造体定義は、`src/model/sheet.rs`に存在する(直接確認済み)にもかかわらず、`model/sheet.md`自身の「主要な型」コードサンプルには一度も示されておらず、散文でのみ説明されている。`Sheet::images`/`col_width_ranges`が実際に`json.rs`でどうシリアライズされるかについても同じギャップがある。

  これが本レビューにとって特に重要なのは、Finding 1がまさにこの種の新規追加された数値/ネストロジックに関するものだからである——設計文書のコードサンプルのみに基づいて作業するレビュアー(本レビューの依頼内容が想定していた通り)は、`i64`のEMUフィールドの境界チェック方針(文書化されておらず、実際には通常の`i64`パース成功以上のチェックは強制されていない——下記の注記参照)や`group_stack`/`resolve_grouped_pic`の形状そのものを、散文を節ごとに読む以外の方法では確認できない——通常レビュアーが権威あるインターフェース要約として扱うはずの「主要な型」ブロックからは見えない。Finding 1のギャップと本ドキュメントギャップが同じモジュールに存在するのは、おそらく偶然ではない。

  別件として、それ自体はfindingとするほどのリスクではないが: `AnchorMarker::col_off`/`row_off`と`ImageExtent::cx`/`cy`(EMU単位)は`parse_attr_i64`(`src/parse/drawing.rs`で確認済み)経由でパースされ、「`i64`としてパースできる」以上の範囲チェックは一切無い——攻撃者は`cx="9223372036854775807"`を設定でき、それは変更されずにJSON出力へプレーンな整数として伝播する。`serde_json`は大きな`i64`値を(指数表記ではなく)リテラルな整数としてレンダリングするため、`JSON.parse`を使うJavaScript側の消費者は2^53を超えた時点で精度を静かに失う——これは本ライブラリ自体の脆弱性ではなく(`resolve_grouped_pic`内のいかなる内部計算も、スケール係数が`f64`で計算されるためこれによってオーバーフローすることはない)、下流の消費者にとってのデータ忠実性上の落とし穴であり、`model::CellRef`の行/列境界チェックが別のフィールドについて既に対処したのと同じ設計上のトレードオフの範疇にあると言えるが(`docs/security/old/code-review.md` Finding 2)、現状`model/sheet.md`の未決事項には文書化されていない。

* **リスクシナリオ**: それ単体では直接悪用可能ではない。リスクはプロセス・検証可能性の面にある——将来の設計文書のみに基づくセキュリティレビュー(または関連機能を実装する新規コントリビューター)が、`model/sheet.md`や`json.md`のコードサンプルを単独で読み、それらのサンプルが関連する型の完全かつ最新の全体像であると合理的に結論づけてしまい、まさにFinding 1が説明するようなギャップを見逃す可能性がある。

* **推奨事項**: `Image`/`ImageAnchor`/`AnchorMarker`/`ImageExtent`/`ColWidthRange`の構造体定義を`model/sheet.md`の「主要な型」コードブロックへ反映する(既に完全に実装され安定している`src/model/sheet.rs`からほぼそのままコピーできる)、対応する`images`/`columns`のシリアライズコードを`json.md`の「主要な型」ブロックへ追加する、`model/mod.md`の再エクスポート一覧に不足している型を追加し`src/model/mod.rs`の実際の再エクスポートと一致させる。その際、`model/sheet.md`の未決事項に、EMUフィールドの大きさに上限が無いことと、それに起因する大きな整数のJSON精度上の注意点を(結論が「許容する、脆弱性ではない」であっても——`docs/security/old/code-review.md` Finding 2が`CellRef`について扱ったのと同様に)一行追記すること。

## 設計上妥当と判断した点(findingとしては数えないが、根拠とともに記録する)

* **XXE・Zip Bomb・Zip Slipの緩和が新規モジュール全てにきれいに拡張されている** ([`parse/drawing.md`](../design/parse/drawing.md)、[`parse/styles.md`](../design/parse/styles.md)、[`pipeline.md`](../design/pipeline.md)フェーズ3.5): 新設された2つの`parse/`モジュールはいずれも`Reader`を`create_secure_reader`経由でのみ構築し、イベントを`read_event`経由でのみ読む(各ファイル自身の「主要な型」コードとエラー処理方針節で確認済み——`drawing.md`は「構文的に不正なXMLは、他の`parse/`モジュールと同じ`create_secure_reader`/`read_event`ゲートウェイを経由して`Error::XmlParse`/`Error::ZipBombDetected`/`Error::DoctypeRejected`に変換される」と明示しており、`styles.md`も同様)。`pipeline.md`のフェーズ3.5は、他の全てのOPCパーツと同じ`container::get_entry`/`parse::parse_relationships`ゲートウェイを通じて`drawingN.xml`/`drawingN.xml.rels`を特定・読み込み、`Internal`な画像ターゲットの存在のみを`container::ZipContainer::has_entry`経由でチェックする(バイト列を一切読まない呼び出し元のために、不要な`get_entry`/`BoundedReader`構築を避ける目的で追加された)——`validate_entry_path`による再検証を一度も迂回しない。並行する、より緩い読み取り経路を記述・示唆する新しい設計文書は見つからなかった。
* **`resolve_grouped_pic`の`chExt=0`除算ゼロガード** ([`parse/drawing.md`](../design/parse/drawing.md)エラー処理方針、`src/parse/drawing.rs`の`resolve_grouped_pic`で確認済み): 本レビューが確認を求められていた数値安全性チェックそのもの——`scale_x = ext_cx / ch_ext_cx`は、`ch_ext_cx == 0 || ch_ext_cy == 0`に対する明示的な`Error::InvalidPackage`によるfail-fastでガードされており、いずれの除算が実行される前にもチェックされる。「整形式だが意味を成さない数値を`NaN`/`Infinity`として静かに生成するのではなく拒否する」という本ファイルの一般的な方針と一貫している。
* **前回レビューで指摘された日付書式ヒューリスティックのReDoS懸念は実装レベルで完全に解消されている** ([`parse/styles.md`](../design/parse/styles.md) `is_date_time_format`、オープンクエスチョン2; `docs/security/old/design-review.md` Finding 4): `src/parse/styles.rs::contains_date_time_token`で直接確認済み——バックトラッキングを一切伴わない`Chars`の単一パス走査(角括弧・引用符・バックスラッシュエスケープの処理はいずれも同じイテレータに対する前方のみの`for`ループであり、再走査は無い)であり、正規表現ではない。設計文書自身のオープンクエスチョン2は分類の*精度*(偽陽性/偽陰性率)についてのみ未解決のままであり、旧findingが提起した計算量の論点については解決済み——この解決を明示的に相互参照する短い注記を追加する価値はあるが、現存するリスクではない。
* **`resolve::column_width`が結合セルfindingの教訓を、自身のアルゴリズムには必要無い箇所にも取り入れている** ([`resolve/column_width.md`](../design/resolve/column_width.md)「`resolve/merge.rs`との関係」および設計の経緯節): `resolve/column_width.rs`の1次元区間重複チェック自体にはO(N²)/O(N³)のリスクが無い(一度ソートして隣接ペアのみをチェックすれば十分)にもかかわらず、設計文書は`MAX_MERGE_REGIONS`が存在するのと同じ理由で件数上限(`MAX_COLUMN_WIDTH_RANGES = 2,000`)を明示的に設けている——「ファイル形式がそれを妨げないことと、それがタダで済むことは別問題」という、前回レビューサイクルの教訓を新規コードへ直接かつ理にかなった形で適用した例であり、まさにFinding 1が示すような防御的上限の考え方が、いまだ一律には適用されていないことの裏返しでもある。
* **`parse/drawing.rs`と`parse/styles.rs`にわたる一貫したfail-closed/段階的縮退の使い分け** ([`parse/drawing.md`](../design/parse/drawing.md)、[`parse/styles.md`](../design/parse/styles.md)エラー処理方針節): 新設された2つのモジュールはいずれも、本プロジェクトの他所で既に確立された二層方針を適用している——本当に壊れた文書(必須要素の欠落、パース不能な数値属性、構文的に不正なXML)は型付きの`Error`でfail-fast拒否する。意味的には曖昧だが整形式な値(`styles.xml`内の解決不能な`numFmtId`/`fontId`/`fillId`/`theme`/`indexed`参照、画像を持たないアンカー)は、エラーにはせず文書化された既定値へ段階的に縮退する。いずれのモジュールも第3の、一貫性の無い方針を持ち込んでおらず、いずれも設計上不正な入力でパニックしない(各ファイルの明示的な「決してパニックしない」旨の記述に対して確認済み)。
* **`Sheet::finalize_merges`のsweep-line修正は正しい層への正しい修正として妥当性を保っている** ([`model/sheet.md`](../design/model/sheet.md)「修正: `finalize_merges`」、[`resolve/merge.md`](../design/resolve/merge.md)オープンクエスチョン2の第2追記): 本レビューは、Issue #43の修正が`Sheet::get`/`iter_cells`の1セルあたりの解決コスト(実際のO(セル数×結合領域数)のハザード)を正しく標的にしたのであって、`resolve/merge.rs`自身の(既に上限のある)O(N²)検証ループではないことを再確認する(再検証はしない)。3つのより単純な、計測の上で却下された代替案(グローバルなカットオフ、行バケット化、区間木)を経てsweep lineへ着地したという設計文書自身の記録は、「推測せず計測する」という本プロジェクトの規律の良い手本であり、Finding 1はこれを今後グループ化画像のネストにも向けるべきことを示している。

## 対象外

* `quick-xml`・`zip`・`serde`/`serde_json`・`thiserror`のサプライチェーン/依存関係の脆弱性——前回までのレビューと同様、対象外。
* セキュリティ上の意味を持たない純粋なコード品質・アーキテクチャ上の懸念(`Relationship.rel_type`を`enum`にすべきか、`model/workbook.md`の線形なシート名検索、MSRV方針等)——設計文書内にはこの種の未決事項がいくつか残っているが、いずれもZip Bomb/Zip Slip/XXE、攻撃者制御下の計算量、未検証の数値伝播、fail-closedの一貫性のいずれにも関わらないため、本レビューでは扱わない。
* 前2回のレビューで既に解決され、それ以降変化していないfinding(XXEの`read_event`ゲート、Zip Bombの`SizeLimits`/`BoundedReader`、CSV/数式インジェクションのREADME警告、結合セルのO(N²)修正そのもの)は再検討しない——それぞれが新規追加モジュールにわたって引き続き健全であることをどう再確認したかは総合評価を参照(元の分析の繰り返しはしない)。
* `docs/design/container/sanitize.md`オープンクエスチョン4(圧縮率に基づく早期スクリーニング)およびオープンクエスチョン5(エントリ名検証のallowlist方式かdenylist方式か)は、最初のレビューが残した状態のまま未解決であり、2026-08-17以降これらに関する新しい情報は無い——本レビューでは再評価しない。
* `Image::hyperlink`がグループレベル(個々の画像単位だけでなく)のハイパーリンクも反映すべきか、`editAs`アンカー挙動属性、その他`parse/drawing.md`内の純粋な忠実性に関する未決事項——セキュリティ上の意味は無い。
* `src/`の行単位の完全な監査は、この設計文書中心のレビューの対象外である(Finding 1は、設計文書自身の記述が上限の不在について検証可能な主張をしていたために、狙いを定めて実測した)。これは並行して行われている想定の、新規実装モジュールに対する専用のコードレベルレビューの代替にはならない。
