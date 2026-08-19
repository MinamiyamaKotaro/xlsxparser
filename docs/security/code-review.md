# `src/` セキュリティコードレビュー (2026-08-19)

*[English](code-review.en.md)*

`docs/security/old/code-review.md`は2026-08-17時点(PR #30後)の`src/`をレビューしたものだった。本レビューはその後継として、`master`上の現在の`src/`ツリー全体——23ファイル・10,258行、前回レビューが見ていなかった複数の機能追加の波(画像アンカーとグループ化画像(Issue #65/#67)、`styles.xml`からのフォント/折返し/配置/書式コード/塗りつぶし色の解決(Issue #37/#38/#41/#42/#75))を含む——を対象にした静的解析(SAST)レビューである。前回同様、OWASP Top 10の観点にとどまらずRust固有の懸念(`unsafe`、パニック安全性、整数オーバーフロー、計算量)もスキャンしており、以下の各findingはいずれも実際にコードを実行して再現したものであり、読解のみによる推測ではない。

## 総合評価

旧レビューのFinding 1・2は引き続き修正済みで退行していない(確認済み: [`src/resolve/merge.rs`](../../src/resolve/merge.rs)の`MAX_MERGE_REGIONS = 20_000`、[`src/model/cell.rs`](../../src/model/cell.rs)の`CellRef::MAX_ROW`/`MAX_COL`によるクランプ)。同じ規律は[`src/parse/drawing.rs`](../../src/parse/drawing.rs)の新設`zero_based_to_cell_ref`にも正しく*拡張*されており、そのドキュメントコメントは旧Finding 2の論拠を明示的に引用している。`unsafe`は`src/`のどこにも一切存在せず、`cargo audit`もクリーン(30クレート走査)である。

しかし本レビューでは、旧レビューのFinding 1と全く同じ種類の問題——今回は`src/parse/drawing.rs`の`<xdr:grpSp>`ネスト/画像解決経路——を発見・実測した。加えて、旧Finding 2のパターン(境界チェックの無い攻撃者制御可能な数値が公開APIへ到達する)の新しいインスタンスも、今回はEMU座標計算において発見した。いずれも、本レビュー作成の一環として、旧レビューのFinding 1・2が確立した手順と全く同じ形で、発見・実測・修正・再計測による確認まで完了させた。1件の情報提供のみのfinding(`f64`の有限値方針の不整合、クラッシュはしない)は未対応のまま残しており、これは旧レビューが自身のFinding 3(`check_end_names`依存)を情報提供のみとして残した扱いと同じである。

## Findings

### ~~Finding 1: `<xdr:grpSp>`ネスト深さの無制限により1画像あたりの解決がO(深さ)になり、小さなdrawing.xmlでO(N²)のCPU枯渇DoSを引き起こす~~ → **解決済み**

* **脆弱性の種類**: 計算量に起因するサービス拒否(CWE-407 Algorithmic Complexity / OWASP API4:2023 Unrestricted Resource Consumption)——旧レビューの解決済みFinding 1と同じ種類、今回は別モジュールで発生。
* **深刻度**: ~~High~~ → 解決済み
* **対象**: [`src/parse/drawing.rs`](../../src/parse/drawing.rs)の`parse_anchor_body`(`group_stack: Vec<GroupContext>`へpushする`b"grpSp"`分岐)、および`</xdr:pic>`ごとに1回呼ばれる`resolve_grouped_pic`。
* **発見時点での詳細**: `group_stack`は`<xdr:grpSp>`の開始ごとにpush、終了ごとにpopされていたが、ファイル内のどこにも深さの上限が無かった。`resolve_grouped_pic`は画像を1枚解決するごとに`group_stack`*全体*を逆順に走査する——O(現在のネスト深さ)の操作である。`<xdr:grpSp>`はスキーマレベルのネスト制限無しに任意の深さで任意個の兄弟`<xdr:pic>`を持てるため、`D`段のネストと最内層に`N`枚の兄弟画像を持つdrawingパーツの総コストは**O(N × D)**になる一方、それを構築するのに必要なバイト数はO(N + D)にすぎない——旧レビューの結合セルfinding(バイト数上限が攻撃者制御可能な軸を制限しないことに隠れたO(N²)コスト)と同じ形だが、今回は1つの件数の自乗ではなく、互いに独立して安価に膨張させられる2つの軸の掛け算である。

  計測結果(releaseビルド、Apple系ハードウェア、シングルスレッド。計測後に元へ戻した一時的な`#[ignore]`付きテストによる):

  | ネスト深さ(D) | 最内層の画像枚数(N) | XMLサイズ | 所要時間 |
  | ---: | ---: | ---: | ---: |
  | 100 | 100,000 | 29.8 MB | 191 ms |
  | 100,000 | 100 | 15.5 MB | 193 ms |
  | 5,000 | 5,000 | 2.26 MB | 135 ms |
  | 20,000 | 2,000 | 3.69 MB | 222 ms |
  | 50,000 | 1,000 | 8.0 MB | 325 ms |
  | 100,000 | 5,000 | 17.0 MB | **2.33 秒** |
  | 50,000 | 50,000 | 22.6 MB | **10.9 秒** |

  どちらか一方の軸のみでは安価なままであり、その積のみが高コストになる点がO(N × D)の形を裏付けている。別途独立した実測(グループ階層あたり多数の小さな兄弟画像、D=Nとして同時にスケール)では、nを2倍にするごとにきれいに**4倍**という、教科書的なO(n²)の挙動に収束し、これほど反復の多い内容はZIPコンテナ内でおよそ90倍に圧縮されると外挿されることから、**圧縮後約410KBの`.xlsx`で約60秒**、**約1〜1.5MBで約10分**のブロッキングを引き起こしうると推定された——いずれも512MiBの1エントリあたりZip Bomb上限には全く届かない。

* **攻撃シナリオ**: 攻撃者が、深くネストした`<xdr:grpSp>`ツリーと最内層の兄弟画像を持つ`drawingN.xml`を含む`.xlsx`(または`parse_workbook`/`parse_workbook_reader`を未検証入力に対して呼ぶあらゆるシステム、例えば文書アップロード機能)を送信する——数十KB〜数MB規模で、既存のあらゆるチェック(Zip Bomb・Zip Slip・XXE・`MAX_MERGE_REGIONS`・`MAX_COLUMN_WIDTH_RANGES`)のどれにも引っかからずに通過する。呼び出し元スレッドは`parse_drawing`内で数秒〜場合によっては数分ブロックされ、少数の同時リクエストでスレッド/ワーカープールを枯渇させられる。

* **解決内容**: `src/parse/drawing.rs`に`pub(crate) const MAX_GROUP_NESTING_DEPTH: usize = 64`を追加し、`parse_anchor_body`の`b"grpSp"`開始タグ分岐内でチェックする——上限を超えて`group_stack`を積もうとするグループ開始タグは、そのグループのそれ以上の内容を読む前に新設した`Error::TooManyNestedGroups { path, depth, limit }`([`src/error.rs`](../../src/error.rs)、`Error::TooManyMergedRanges`/`Error::TooManyColumnWidthRanges`と同じ形)を返す。実際のグループネストはほぼ常に数段程度にとどまるため、64は正当なファイルに対して十分な余裕を残しつつ、最悪ケースのコストをO(N × 64)に抑える。

  回帰テストを追加: `group_nesting_depth_at_the_limit_is_accepted`/`group_nesting_depth_over_the_limit_is_too_many_nested_groups`が64/65のちょうど境界をカバーする(`resolve/merge.rs`の`region_count_at_the_limit_is_accepted`/`region_count_over_the_limit_is_too_many_merged_ranges`のパターンに倣う)。

  修正後にD=100,000/N=100の攻撃形状を再計測: **1ミリ秒未満で拒否**——O(N × D)の変換処理が一切走る前に、深さチェックがネスト段数65で発火する。

### ~~Finding 2: ネストしたグループのEMU座標計算が無制限で、わずかな細工ファイルでアンカーのオフセット/サイズが公開JSON APIへ到達する前に`i64::MAX`まで到達する~~ → **解決済み**

* **脆弱性の種類**: 下流へ伝播する入力検証の欠落(CWE-1284)——旧レビューの解決済みFinding 2(`CellRef`の行/列)と同じパターン、今回は画像アンカーのEMU座標について発生。
* **深刻度**: ~~Medium~~ → 解決済み
* **対象**: [`src/parse/drawing.rs`](../../src/parse/drawing.rs)の`resolve_grouped_pic`——`model::AnchorMarker.col_off`/`row_off`(`i64`)と`model::ImageExtent.cx`/`cy`(`i64`)に供給され、`json.rs`がこれをそのまま公開JSONフィールドとしてシリアライズする。
* **発見時点での詳細**: `resolve_grouped_pic`は`ch_ext_cx == 0 || ch_ext_cy == 0`(即座の除算ゼロを回避)のみをガードしており、レベルごとのスケール係数(`ext_cx/ch_ext_cx`)がどれだけ大きくなりうるか、また何段分のスケール係数が繰り返し乗算で複合するかについては一切上限が無かった。わずか2段のネストした`<xdr:grpSp>`(それぞれ`chExt cx="1" cy="1"`、`ext cx="9223372036854775807" cy="9223372036854775807"`(`i64::MAX`))を持つ細工した`<xdr:oneCellAnchor>`は、エラー無く全フィールドが`i64::MAX`に飽和した状態へ解決された: 十分な段数の後にスケール係数の累積積が`f64::MAX`を超え`f64::INFINITY`になり、`Infinity`に対する`.round() as i64`はパニックしない——Rustの飽和的なfloat→int変換が静かに`i64::MAX`を生成する。この経路のどこにもエラーは発生せず、値はそのまま公開JSON出力へ流れ込む——旧Finding 2が偽造された`CellRef`について`maxRow`/`maxCol`で説明したのと全く同じ形。必要な入力はごくわずか(5KB未満)だった。
* **攻撃シナリオ**: この細工した`drawing1.xml`を持つファイルは正常にパースされ、JSONの`images[].anchor.from.colOff`/`rowOff`/`ext.cx`/`ext.cy`に攻撃者が選んだ巨大な値を生成する。これらのEMU値を物理的に妥当なものとして信頼する下流の消費者(例えば報告された大きさに合わせてレンダリング用バッファを確保する処理)は、旧Finding 2が素朴な`maxRow`/`maxCol`の消費者について実演したのと同種のOOM/クラッシュへ追い込まれうる。
* **解決内容**: `resolve_grouped_pic`の最終的な解決後の`(x, y, cx, cy)`について、`is_finite()`および防御的な妥当性上限`MAX_PLAUSIBLE_EMU = 1_000_000_000_000.0`(10^12 EMU、約27.7km——Excelの実際の最大シート範囲を大きく上回る)に対するチェックを追加し、いずれかに失敗した場合は`Error::InvalidPackage`を返すようにした——既存の`chExt == 0`ガードが既に採用している「整形式だが意味を成さない数値を静かに丸め込むのではなく拒否する」という方針と同じ。回帰テストを追加: `extreme_group_transform_scale_is_rejected_as_invalid_package`。

### Finding 3(情報提供): `tint`/フォントサイズ/列幅の`f64`フィールドが、本コードベースの他所で適用されている有限値方針を適用されておらず、静かにJSON `null`へ縮退する

* **脆弱性の種類**: 入力検証方針の不整合(CWE-1284に最も近い、情報提供レベル——クラッシュしないことは確認済み)。
* **深刻度**: Low / 情報提供のみ
* **対象**: [`src/parse/styles.rs`](../../src/parse/styles.rs)の`parse_color`内の`tint`パース、および`<sz val="..">`のパース; [`src/model/style.rs`](../../src/model/style.rs)の`ColorRef::Theme.tint: Option<f64>`/`Font.size_pt: f64`; [`src/model/sheet.rs`](../../src/model/sheet.rs)の`ColWidthRange.width: f64`。
* **詳細**: `f64::from_str`はリテラル文字列`"nan"`/`"inf"`/`"-inf"`、および無限大へオーバーフローする数値リテラル(例: `"1e400"`)を受け付けるため、細工した`<fgColor theme="4" tint="nan"/>`や`<sz val="1e400"/>`は非有限な`f64`を生成し、それが`ResolvedStyle`へ無検証のまま流れ込む。`serde_json`が導出する`f64`/`Option<f64>`の`Serialize`はこれに対してエラーもパニックもしない——静かにJSON `null`を出力し、「属性が本当に存在しない」場合と区別が付かなくなる。これは安全である(クラッシュもDoSも無い)が、本コードベース自身が`json.rs`で`CellValue::Number`に対して明示的に行っている扱い(専用の回帰テスト`non_finite_numbers_fall_back_to_empty_without_erroring`を伴い、NaN/Infinityを静かに丸め込むことがなぜ下流の消費者にとって望ましくないかを明記している)とは不整合である。
* **リスクシナリオ**: データ忠実性上の細かな話であり、悪用可能な脆弱性ではない。下流の消費者は`"tint": null`を見て、「このセルにはtintが無い」のか「このセルのtintフィールドが攻撃者によって汚染された」のかを区別できない。
* **推奨される修正**: `CellValue::Number`の方針との一貫性のため、非有限な`tint`/`size_pt`/`width`をパース時点で拒否する(`None`/`Font::default()`へフォールバックする、または範囲をスキップする——パース不能な`numFmtId`/`fontId`/`fillId`に既に使われている段階的縮退方針と同じ)か、これらのフィールドが「属性が存在しない」以外の理由で`null`としてシリアライズされうることを明示的に文書化する。本レビューサイクルでは対応せず未対応のまま残す——旧レビューが自身のFinding 3(`check_end_names`依存)を情報提供として残した扱いと同じ。

## 良好だった点

* `src/`のどこにも**`unsafe`コードが一切存在しない**——旧レビューから変化なし。
* **`cargo audit`はクリーン**——30クレート走査、既知の脆弱性なし。
* **Zip Bomb・Zip Slip・XXEの防御が、追加された全ての新規OOXMLパーツに一様に適用されている。** `xl/drawings/drawingN.xml`、`xl/drawings/_rels/drawingN.xml.rels`、`xl/styles.xml`はいずれも、既存の全パーツと同じ`container::get_entry` → `create_secure_reader` → `read_event`のゲートウェイ連鎖のみを経由して読まれる——`pipeline.rs::resolve_sheet_images`や`parse/drawing.rs`/`parse/styles.rs`に、新たな直接ファイル読み取りによる迂回経路は見つからなかった。
* **旧Finding 1・2は引き続き修正済みで退行しておらず**、Finding 2の規律は正しく拡張されている: `parse/drawing.rs`の`zero_based_to_cell_ref`は自身のドキュメントコメントで旧findingの論拠を明示的に引用しており、`out_of_range_row_is_invalid_cell_ref`でカバーされている。
* **グループのネストはネイティブなスタックオーバーフローのリスクを伴わない。** `<xdr:grpSp>`のネスト(Finding 1)はヒープ確保された`Vec<GroupContext>`で追跡されており、実際の関数再帰ではない——`parse_anchor_body`は単一のフラットなループであり、`quick_xml`のトークナイザ自体も非再帰的である。そのためCPU時間の爆発は実在した(現在は修正済み)ものの、攻撃者がXMLのネスト深さを通じて追加でネイティブなスタックオーバーフローを引き起こすことはできない。
* **`parse/styles.rs`のフォント/塗りつぶしのルックアップはいずれも`.get()`ベースで段階的にフォールバックしており**、直接インデックスは一切無い: 範囲外の`fontId`/`fillId`はパニックせず`Font::default()`/`Fill::default()`へ縮退し、既存の`numFmtId`の方針を踏襲している。fonts/fillsの`Vec`にO(N²)の増大パターンは無い。
* **`is_date_time_format`のカスタム書式コードスキャン**は、攻撃者制御可能な`formatCode`テキストに対するバックトラッキング無しの単一パス線形走査であり、旧設計レビューのFinding 4(ReDoS懸念)を実装レベルで完全に解消している——正規表現はどこにも無い。
* **`Sheet::finalize_merges`のsweep-lineアルゴリズム**(Issue #43で追加)は真にO((C+M) log(C+M))のソート&スイープであり、Excelの実際の最大サイズの結合ですらハングせずに登録できることを証明する回帰テストに裏付けられている。
* **`resolve/style.rs`の日付シリアル値変換**は、いかなる算術を行う前にも明示的に`is_finite()`をチェックし範囲を制限している——偽造された`<v>`セル値は未定義動作へ伝播するのではなく、段階的に縮退する。
* **パイプラインの画像リレーションシップ解決**(`pipeline.rs::resolve_sheet_images`)は厳密にO(画像数)である——全てのリレーションシップ検索は単一の`HashMap::get`であり、二次関数的なパターンは無い。

## 検証方法

Finding 1・2の計測(修正前・修正後とも)は、`src/parse/drawing.rs`に直接追加した一時的な`#[cfg(test)]`/`#[ignore]`関数(クレート内部の`parse_drawing`/`resolve_grouped_pic`経路をreleaseビルドで直接呼び出す)によって行い、各計測の直後に元へ戻した——`git status`/`git diff`で、テストのみを目的とした変更が`src/`に一切残っていないことを確認済み。各findingの「解決内容」に記載した回帰テストが、実際にコミットされたものである。
