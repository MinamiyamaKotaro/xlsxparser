# `parse/drawing.rs` 設計書

*[English](drawing.en.md)*

`src/parse/drawing.rs` に対応する設計書。Issue [#65](https://github.com/MinamiyamaKotaro/xlsxparser/issues/65)(「画像のアンカー位置・リンク先を取得できない」)およびIssue [#67](https://github.com/MinamiyamaKotaro/xlsxparser/issues/67)(「グループ化された画像(`<xdr:grpSp>`)が黙って誤って扱われる」)のうち、純粋なXMLパース部分を担う: `xl/drawings/drawingN.xml` の `xdr:twoCellAnchor`/`xdr:oneCellAnchor` 要素——`<xdr:grpSp>`グループ図形にどれだけ深く入れ子になっていても、その中の全ての`<xdr:pic>`を含めて——を `PendingImage` にパースする。各要素が持つ `r:embed`/ハイパーリンクの `r:id` の解決や、`drawingN.xml` 自体をワークシート自身の `_rels` 経由で特定する処理は `pipeline.rs` の責務([pipeline.md](../pipeline.md) のPhase 3.5参照)であり、[relationships.md](relationships.md) が既に確立していた「ルーティング用データのパース」と「その解釈・解決」の分業に倣う。

## 責務・スコープ

- `xl/drawings/drawingN.xml` の `xdr:twoCellAnchor`/`xdr:oneCellAnchor` 要素をパースする。各要素は1つ以上の `<xdr:pic>`(埋め込み画像)をセル位置に紐付ける——アンカー直下、または1段以上の`<xdr:grpSp>`グループ図形に入れ子になった状態のいずれも(Issue #67)
- 各アンカーについて、`xdr:from`/`xdr:to` マーカー(`TwoCell`)または `xdr:from`/`xdr:ext`(`OneCell`) — セル座標とEMU単位のオフセット。DrawingMLの0始まりの `xdr:col`/`xdr:row` から本クレートの1始まりの `CellRef` へ変換する
- 見つかった各 `<xdr:pic>`(どれだけ深く入れ子になっていても)について、`r:embed`(埋め込みメディアのrelationship ID)、および存在すれば `a:hlinkClick` の `r:id`(画像自体のハイパーリンクのrelationship ID) — まだターゲットパスに解決されていない生の文字列として取得する。アンカー直下の画像はIssue #65以来変わらずアンカー自身の`from`/`to`/`ext`を位置・サイズとして使うが、1段以上の`<xdr:grpSp>`に入れ子になった画像は、囲むグループの`<a:xfrm>`変換を通して位置・サイズを**解決**する(下記`resolve_grouped_pic`参照)——アンカー自身の`from`/`to`/`ext`はグループ全体の外接矩形しか示しておらず、内部の個々の画像の位置は示していない
- アンカー、またはグループの中身が画像でない場合(単純な図形・グラフ・コネクタ、または空のグループ)は無視する(何も返さない)。本Issueのスコープ外
- **含まない責務**: `embed_r_id`/`hyperlink_r_id` を `drawingN.xml.rels` に対して解決すること(`pipeline.rs` の責務。本モジュールは渡された単一のreader以外に2つ目のZIPエントリを開いたり、いかなるI/Oも行わない)、どの `drawingN.xml` がどのワークシートに属するかの特定(`pipeline.rs` が、ワークシート自身の `_rels` と、`parse/worksheet.rs` が新たに収集するようになった `<drawing r:id="...">` 要素経由で行う)、埋め込み画像自体のバイト列を読むこと(Issue全体としてスコープ外 — Issue本文の理由: 差分検出用途のツールにピクセルデータは不要であり、読み込むとメモリ使用量がセル数ではなく画像数に比例してしまう)、グループ化されていない画像にもこの機能追加が及ぼす解析コスト増(別Issue [#71](https://github.com/MinamiyamaKotaro/xlsxparser/issues/71) で追跡)

## 主要な型・関数

```rust
use crate::error::Error;
use crate::model::{AnchorMarker, CellRef, ImageAnchor, ImageExtent};
use crate::parse::{create_secure_reader, optional_attr, read_event, read_leaf_text, required_attr};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::BufRead;

/// `twoCellAnchor`/`oneCellAnchor` 内の `<xdr:pic>` 1個分。relationship ID
/// はまだ実際のターゲットパスに解決されていない。`pipeline.rs` が
/// `embed_r_id`/`hyperlink_r_id` を `drawingN.xml.rels` に対して解決し、
/// `model::Image` に変換する。
pub(crate) struct PendingImage {
    pub anchor: ImageAnchor,
    pub embed_r_id: String,
    pub hyperlink_r_id: Option<String>,
}

/// drawingN.xml 1個分を、それが持つ全ての `<xdr:pic>` にパースする。
pub(crate) fn parse_drawing(reader: impl BufRead, path: &str) -> Result<Vec<PendingImage>, Error> {
    // 各 <xdr:twoCellAnchor>/<xdr:oneCellAnchor> について、その
    // <xdr:from>/<xdr:to>/<xdr:ext> マーカーをパースし、<xdr:pic> が
    // 存在すれば <a:blip r:embed>/<a:hlinkClick r:id> も取得する。
    // <xdr:pic> を持たないアンカーはスキップする(内部的にOk(None)として
    // 扱われ、結果からは単に除外される)。
    ..
}
```

`AnchorMarker`/`ImageExtent`/`ImageAnchor`/`Image` 自体は [`model/sheet.rs`](../model/sheet.md) に `MergedRegion`/`ColWidthRange` と並んで定義されている — 本モジュールは `model::Image` にまだ無い部分(未解決の生のrelationship ID)のみを生成する。これは `parse/worksheet.rs` が解決済みの `Cell` を直接生成するのではなく `PendingSharedString`/`PendingStyle` を生成するのと同じパターンである。グループ画像対応(Issue #67)で`model::`側の変更は一切不要だった: 解決済みのグループ内画像は、囲むアンカーが`TwoCell`か`OneCell`かに関わらず常に`ImageAnchor::OneCell { from, ext }`(明示的なセル+オフセット+サイズ)になる——グループ内の画像は自分自身の`to`マーカーを持たず、解決済みの位置とサイズだけが存在するため。

### グループ画像: `GroupContext`と`resolve_grouped_pic`(Issue #67)

```rust
/// `<xdr:grpSp>` 自身の `<xdr:grpSpPr><a:xfrm>`。
#[derive(Debug, Clone, Copy, Default)]
struct GroupContext {
    off_x: i64,
    off_y: i64,
    ext_cx: i64,
    ext_cy: i64,
    ch_off_x: i64,
    ch_off_y: i64,
    ch_ext_cx: i64,
    ch_ext_cy: i64,
}
```

`parse_anchor_body` は `group_stack: Vec<GroupContext>` を保持し、`<xdr:grpSp>` の開始でpush、終了でpopする——`<xdr:grpSp>` は(`twoCellAnchor`/`oneCellAnchor`と異なり)自己入れ子**可能**なため、スタック自身の長さがそのままネスト深さのトラッカーを兼ねる。整形式のXMLは入れ子要素を常にLIFO順で閉じるため、別途のカウンタは不要である。

`resolve_grouped_pic(group_stack, pic_off, pic_ext)` は各段の線形変換——`child' = off + (child - chOff) * (ext / chExt)`——を最深段から最親段へ向けて適用する(`group_stack` はパース中に最親段から順にpushされるため、逆順に走査する)。ただし1つ例外がある: **最親グループ自身の`off`は除外**する(0として扱う)。これはアンカー自身の`from`点と一致するとみなすためである。得られたデルタを`from.col_off`/`from.row_off`(アンカー自身の`from`マーカー。アンカーのグループツリーが含む全ての画像の基準セルとして再利用される)に加算して最終的な`AnchorMarker`を得る。サイズ(`ext`)も同様に段ごとにスケールされる。

この「最親グループの`off`を除外する」というルールは、実際のLibreOffice出力に対して独立に検証済みである(Issue #67のレビュー議論): 最親グループの`off`/`ext`は(当初疑われた「`from`相対の値」ではなく)文字通り絶対キャンバスEMU座標である——しかし`from`自身の真の絶対位置とグループの`off`は、構成上、全く同じ物理的な点を指している(そうでなければファイルは正しく描画されない、という幾何学的必然)ため、**デルタ**だけが必要な場面ではこの2つは相殺される。行高・列幅のルックアップは一切不要であり、これは合成的な3段ネストのケースに対してアルゴリズムを手でトレースすることと、実際のLibreOfficeサンプルの実数値に対してこれを実行することの両方で確認済みである(`nested_group_resolves_correctly`/`single_level_group_resolves_each_pic_relative_to_from`テスト参照)。

### 0始まりから1始まりへの変換

DrawingMLの `xdr:col`/`xdr:row` は0始まり(ECMA-376 Part 1の `ST_ColumnRow` — `CT_Marker` の祖先)だが、本クレートの `CellRef` はA1形式に合わせて1始まりである([`model/cell.md`](../model/cell.md) 参照)。`zero_based_to_cell_ref` は `CellRef` を構築する前に各値へ1を加算し、変換後に `u32` をオーバーフローする、または `CellRef::MAX_ROW`/`MAX_COL` を超える値は `Error::InvalidCellRef` として拒否する。これは `CellRef::from_a1` 自身の範囲チェックと同じ理由(セキュリティレビュー `docs/security/code-review.md` Finding 2)によるもので、どの経路から来た座標であってもXML由来の攻撃者制御可能な値が未検証のままモデルに到達してはならない、という原則に従う。

## 依存関係

- 依存先: [`parse/mod.rs`](mod.md)(`create_secure_reader`, `read_event`, `read_leaf_text`, `required_attr`, `optional_attr`)、[`model/sheet.rs`](../model/sheet.md)(`AnchorMarker`, `ImageAnchor`, `ImageExtent`)、[`model/cell.rs`](../model/cell.md)(`CellRef`)、[`error.rs`](../error.md)
- `read_leaf_text` は元々 `parse/worksheet.rs` 内のprivateヘルパーだったが、本モジュールからも使うために `parse/mod.rs` の共有関数へ昇格させた — 両モジュールとも「ネストした要素を想定しない」単純な数値・テキストのleaf要素(`<v>`, `<xdr:col>` 等)を読む点で共通しており、`concat_rich_text` が扱うより複雑な `<r><t>` ラン構造とは性質が異なる
- 依存元: `pipeline.rs` のPhase 3.5([pipeline.md](../pipeline.md) 参照)。`PendingImage` のrelationship IDを `drawingN.xml.rels` に対して解決し、最終的な `Vec<model::Image>` を構築する

### `<xdr:ext>`と`<a:ext>`のローカル名衝突(Issue #65の追加修正)

`parse_anchor_body`のフラットな接頭辞非依存のイベントスキャン([parse/mod.md](mod.md)の名前空間方針参照)では、`<xdr:ext>`(`OneCell`アンカー自身の表示サイズ、`oneCellAnchor`の直下の子)と`<a:ext>`(`<xdr:pic>`自身の`<xdr:spPr><a:xfrm>`内にあり、図形自体の内部ジオメトリを表す)は、接頭辞を取り除くとどちらも単に"ext"となるためローカル名だけでは区別できません。実際のライター(仮説ではなく実際のLibreOffice出力で確認済み)は、グループ化していない単純な`<xdr:pic>`に対しても`<xdr:spPr><a:xfrm><a:ext>`を出力し、これは文書順でアンカー自身の`<xdr:ext>`より**後**に出現します(`CT_OneCellAnchor`スキーマの`from, ext, pic`という順序)。そのためガードが無いと、pic内部の(通常は無関係な)サイズが、実際に画面に表示されるアンカーのサイズを静かに上書きしてしまいます。`<xdr:ext>`はまさに差分検出用途の消費者が関心を持つ値(シート上に表示される実際のサイズ)であるため、これは見た目上の問題ではなく実際の正しさに関わるバグでした。

深さカウンタではなく単純な`bool`(`in_pic`。`<xdr:pic>`は自己入れ子しないため十分)を用いて`<xdr:ext>`のマッチを`!in_pic`のときのみ有効にすることで修正しました。グループ画像対応(Issue #67)ではこれを(グループレベル/picレベル/アンカーレベルの)3方向分岐に一般化しました——`<xdr:pic>`自身の`off`/`ext`を実際に読む必要が生じたためですが、それは画像がグループ内にある場合**のみ**です(`in_sp_pr`自体が`in_pic && !group_stack.is_empty()`でガードされている)。グループ化されていない画像では`spPr`に一切入らないため、その`<a:ext>`は引き続き`!in_pic`のアンカーレベル分岐に落ち、以前と同様に正しく無視されます。

Issue #67の実装中に、もう1つ関連するスコーピング問題が見つかりました: `hlinkClick`の捕捉も`in_pic`でガードする必要があります。**グループ自体**に貼られたハイパーリンク(`<xdr:nvGrpSpPr><xdr:cNvPr><a:hlinkClick>`)は、その中の画像より文書順で先に出現するため、ガードが無いと最初に見つかった画像に紛れ込んでしまいます。`embed_r_id`/`hyperlink_r_id`/`pic_off`/`pic_ext`は各`</xdr:pic>`ごとにリセットされるため、グループ内のある画像から次の兄弟画像への漏れも防いでいます。

## エラー処理方針

- `<xdr:pic>` を持つアンカーにおいて必須要素(`TwoCell` アンカーの `xdr:from`/`xdr:to`、`OneCell` アンカーの `xdr:from`/`xdr:ext`、`<xdr:pic>` の `<a:blip>` が持つべき `r:embed`)が欠落している場合は `Error::MissingRequiredElement` — `parse/worksheet.rs` が `<c>` の `r` 属性欠落に適用するのと同じfail-fast方針
- `<xdr:pic>` を全く持たないアンカーは上記チェックが走る前に早期リターンし、結果から単に除外される — 単純な図形・グラフのアンカーがこれらを持たないのは正当なため、エラーとしない
- leaf要素の数値内容が不正な場合(`xdr:col`/`xdr:colOff`/`xdr:row`/`xdr:rowOff`、または `xdr:ext` の `cx`/`cy` 属性)は `Error::InvalidPackage` — `parse/worksheet.rs::parse_u32_attr`/`parse_f64_attr` の規約(整形式の要素だが期待する型としてパースできない内容を持つ場合)に倣う
- `<xdr:pic>` が自身の `<xdr:spPr><a:xfrm><a:ext>` にアンカー自身の `<xdr:ext>` と**異なる** `cx`/`cy` を持つ場合でも、`oneCellAnchor` の解決結果はアンカー自身の値を採用する(pic内部の値では上書きされない。Issue #65の追加修正 — 上記のローカル名衝突の注記参照)
- 0始まりから1始まりへの変換でオーバーフローする、または `CellRef::MAX_ROW`/`MAX_COL` を超える座標は `Error::InvalidCellRef`(上記「主要な型・関数」参照)
- 構文的に不正なXMLは、他の `parse/` モジュールと同じ `create_secure_reader`/`read_event` のゲートウェイを経由して `Error::XmlParse`/`Error::ZipBombDetected`/`Error::DoctypeRejected` に変換される
- `chExt`がいずれかの軸でゼロの`<xdr:grpSp>`(`resolve_grouped_pic`の`ext / chExt`が未定義になる)は`Error::InvalidPackage`でfail-fast——整形式だが意味を成さない数値を`NaN`/`Infinity`として静かに生成するのではなく拒否する、本ファイルの一般的な方針に倣う(Issue #67)
- `MAX_GROUP_NESTING_DEPTH`(64)を超える`<xdr:grpSp>`のネストは`Error::TooManyNestedGroups`——ネストしたグループの開始タグが`group_stack`をこの上限より深く積もうとした時点でチェックし、そのグループの内容をこれ以上読む前に拒否する(セキュリティレビュー Finding 1、Issue #71のフォローアップ。詳細は後述)
- グループ変換の解決後の座標(`resolve_grouped_pic`の最終`x`/`y`/`cx`/`cy`)が非有限(`NaN`/`Infinity`——極端な`ext`/`chExt`比率をネスト段数分掛け合わせることで到達可能)、または防御的な妥当性上限(`MAX_PLAUSIBLE_EMU`、10^12 EMU——Excelの実際の最大シート範囲を大きく上回る)を超える場合は`Error::InvalidPackage`でfail-fast、変換ループ完了後に一度だけチェックする(セキュリティレビュー Finding 2、Issue #71のフォローアップ)

## テスト方針

- `<a:blip r:embed>` と `<a:hlinkClick r:id>` の両方を持つ `twoCellAnchor` が、両方のIDを捕捉し、`from`/`to` マーカーが正しく1始まりの `CellRef` とEMUオフセットに変換された `PendingImage` にパースされることの確認
- `<xdr:ext>` を持ちハイパーリンクを持たない `oneCellAnchor` が、`hyperlink_r_id: None` かつ `ext` の `cx`/`cy` が保持された `PendingImage` にパースされることの確認
- `<xdr:pic>` を持たないアンカー(例: 単なる `<xdr:sp>`)がスキップされることの確認 — `from`/`to` 自体は整形式であっても `PendingImage` を生成せず、エラーにもならない
- 複数のアンカーを持つ `drawingN.xml` から、画像アンカーの数だけ文書順に `PendingImage` が生成されることの確認
- `<a:blip r:embed>` を欠く `<xdr:pic>` が `Error::MissingRequiredElement { name: "r:embed", .. }` になることの確認
- 1始まり変換後に `u32` をオーバーフローする、または `CellRef::MAX_ROW`/`MAX_COL` を超える `xdr:row`/`xdr:col` の値が `Error::InvalidCellRef` になることの確認
- `<xdr:pic>` が自身の `<xdr:spPr><a:xfrm><a:ext>` にアンカーの `<xdr:ext>` と異なる `cx`/`cy` を持つ `oneCellAnchor` で、解決結果がアンカー自身の値になる(pic内部の値に上書きされない)ことの確認(Issue #65の追加修正)
- `xdr:ext` の属性が不正な場合(`cx`/`cy` が数値でない)に `Error::InvalidPackage` になることの確認
- アンカーを一切持たない空の `<xdr:wsDr>` が空の `Vec` を返すことの確認
- 2つの画像を持つ単一段の`<xdr:grpSp>`が、それぞれ独立した`ImageAnchor::OneCell`に解決されること——アンカーの`from.cell`は共有するが`col_off`/`row_off`のデルタとサイズはそれぞれ異なる——実際のLibreOffice出力から採取した実数値で検証済み(Issue #67)
- 3段の入れ子グループが1枚の画像を正しく解決すること——PoCで検証済みのCase 3と同じ合成ケースに対して変換式を手でトレースして確認
- グループ自体に貼られたハイパーリンク(`<xdr:nvGrpSpPr>`の`<a:hlinkClick>`)が最初の画像の`hyperlink_r_id`に紛れ込まないことの確認
- グループ内のある画像に貼られたハイパーリンクが、ハイパーリンクを持たない次の兄弟画像に紛れ込まないことの確認
- `chExt`がいずれかの軸でゼロの`<xdr:grpSp>`が`Error::InvalidPackage`になることの確認
- 画像を持たない図形のみで構成されるグループ(`<xdr:pic>`がどこにも無い)が画像を一切生成しないことの確認
- `<xdr:grpSp>`のネストが`MAX_GROUP_NESTING_DEPTH`ちょうどでは受理され、1段超えると`Error::TooManyNestedGroups`になることの確認(セキュリティレビュー Finding 1)
- 少数のネスト段数に対して細工した`ext`/`chExt`比率を掛け合わせ、解決後の座標を非有限または非現実的な大きさへ到達させると`Error::InvalidPackage`になることの確認(セキュリティレビュー Finding 2)

## 未決事項 / オープンクエスチョン

1. **図形・グラフ等、画像以外の描画オブジェクト**: 現状は静かにスキップされる(`PendingImage` を生成しない)。将来これらも出力モデルに反映する必要が生じた場合(例: `Image` とは別の汎用的な「図形」アンカーとして)、本モジュールのアンカーごとのループにもう一つの返却経路が必要になる — Issue #65の明示的なスコープが画像のみであるため、本設計では扱わない。
2. **`editAs` 等のアンカー挙動属性**: `xdr:twoCellAnchor` の `editAs` 属性(`twoCell`/`oneCell`/`absolute` — 元となるセルがリサイズされた際の図形の挙動)は取得していない。これはExcelの*動的な*リサイズ挙動に影響するものであり、本ライブラリの出力(差分検出用途)が関心を持つアンカーの*現在の*位置・サイズとは別の関心事である — ただし将来「画像そのものが移動した」のか「周囲のセルがリサイズされて画像が追従した」のかを区別する必要が生じた場合は再検討の余地がある。
3. ~~`parse/relationships.rs` がメディア埋め込み用relsに対応する必要があるか~~ → **解決**: [relationships.md オープンクエスチョン1](relationships.md) で未確定だった論点に、Issue #65が回答を与えた — `parse/relationships.rs` の既存の汎用 `_rels` パーサー(`../media/image1.png` のような相対パスに対して既にテスト済み)を、`xl/worksheets/_rels/sheetN.xml.rels`(`drawingN.xml` の特定)と `xl/drawings/_rels/drawingN.xml.rels`(埋め込みメディア・ハイパーリンクのターゲット特定)の両方にそのまま再利用し、当該モジュールへの変更は不要だった。
4. ~~`<xdr:grpSp>`(グループ化された画像)に対応するか、するならどう対応するか~~ → **解決**: Issue #67 — 上記「グループ画像」節参照。グループ内の画像は常に`ImageAnchor::OneCell`に解決される。
5. **Issue #67によってグループ化されていない画像にも追加される解析コスト**: 別Issue [#71](https://github.com/MinamiyamaKotaro/xlsxparser/issues/71) として追跡。実際のLibreOffice出力に対するPoCベンチマークでは、`parse_anchor_body`の`match`が大きくなったことに起因して1シェイプあたり約20%のコスト増が測定された——追加したアームのガード内ロジックはグループ化されていない画像では実行されないが、各XMLイベントに対する「タグ名か」という比較そのものは常に発生するため。絶対値は数百ナノ秒の規模にとどまる(計測したフィクスチャでのワークブック全体のパース時間約190µsと比べれば無視できる)が、画像枚数が非常に多い実ファイルで測定可能な水準になった場合は再検討の余地がある。
6. **グループ自体に貼られたハイパーリンクの`Image::hyperlink`への反映**: グループ内の**個々の画像**ではなく**グループ自体**に貼られたハイパーリンクは、意図的に出力モデルのどこにも反映していない——個々の画像単位のハイパーリンクのみがIssue #65の元々のスコープである。実ファイルでこれに依存するケースが見つかった場合は再検討する。
7. ~~`<xdr:grpSp>`のネスト深さに防御的な上限が必要か~~ → **解決**: 必要だった。`resolve_grouped_pic`は`<xdr:pic>`を1枚解決するごとに現在のネスト深さに比例したコスト(O(深さ))がかかるため、`D`段のネストと最内層の`N`枚の兄弟画像を持つdrawing部品は、構築に必要なバイト数がO(N+D)であるのに対しコストはO(N×D)になる——Zip Bombのバイト数上限だけでは`D`を制限できず、`docs/security/old/code-review.en.md` Finding 1が結合セル数について見つけたのと同じ形。実測(`docs/security/design-review.en.md` Finding 1): 22.6MBのdrawing部品(D=N=50,000)が修正前は10.9秒の同期的CPU占有を引き起こした。`MAX_GROUP_NESTING_DEPTH = 64`(`parse::drawing`)を追加し、各`<xdr:grpSp>`の開始タグでチェックする——`resolve::merge::MAX_MERGE_REGIONS`/`resolve::column_width::MAX_COLUMN_WIDTH_RANGES`と同じ防御的上限のパターン。修正後に同じ攻撃形状を再計測すると1ms未満で拒否されることを確認した。関連する別の数値上限(`MAX_PLAUSIBLE_EMU`)も同時に追加した——詳細は上記エラー処理方針、および`docs/security/design-review.en.md`/`code-review.en.md`のFinding 1・2を参照。
