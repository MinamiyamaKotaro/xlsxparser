# `parse/mod.rs` 設計書

*[English](mod.en.md)*

`src/parse/mod.rs` に対応する設計書。[architecture.md](../architecture.md) が定義する `parse/` の責務「`quick-xml` などXMLパースライブラリへの依存を集約する層」のうち、個々のパーサー（[relationships.md](relationships.md) / [workbook.md](workbook.md) / [shared_strings.md](shared_strings.md) / [styles.md](styles.md) / [worksheet.md](worksheet.md)）に共通する横断的関心事（セキュアな `Reader` 生成、XMLパースエラーの変換、属性取得・リッチテキスト結合の共通ヘルパー）を集約する。[container/sanitize.md エラー処理方針](../container/sanitize.md) が「`parse/mod.rs` のセキュアReaderファクトリに併設予定」としていた変換ロジックの実体（`convert_xml_error`）はこのファイルで確定させる。

## 責務・スコープ

- サブモジュールの宣言（`mod relationships; mod workbook; mod shared_strings; mod styles; mod worksheet; mod theme;`）とクレート内公開型の再エクスポート
- XXE対策を適用済みの `quick_xml::Reader` を生成する唯一の窓口 `create_secure_reader` を提供する。`parse/` 配下の各モジュールはこの関数経由でのみ `Reader` を取得しなければならず、個別に `Reader::from_reader` を呼ばない（architecture.md 「各パーサーが個別に `Reader` を初期化すると設定漏れのリスクがあるため」を実装する）
- `quick_xml::Error` を [`crate::error::Error`](../error.md) へ変換する唯一の窓口 `convert_xml_error` を提供する。[container/sanitize.md](../container/sanitize.md) が定義する `BoundedReader` からの上限超過（Zip Bomb）を検知し `Error::ZipBombDetected` へ変換する処理もここに集約する
- イベント読み取りの唯一の窓口 `read_event` を提供する。`quick-xml` はDTD内部サブセット・外部実体を解決しない非検証型パーサーであり通常の構成でも古典的なXXEは成立しないが、この前提のみに依拠せず、`<!DOCTYPE ...>` 宣言（`Event::DocType`）自体をXMLの構文として検知した時点で無条件に拒否する（fail closed）ことで、パーサーの内部実装や将来のバージョン変更に依存しない明示的・検証可能なXXE対策とする。`parse/` 配下の各モジュールはこの関数経由でのみイベントを読み取り、`Reader::read_event_into` を直接呼ばない（[セキュリティレビュー](../../security/design-review.md) Finding 1を反映）
- 必須属性の取得（欠落時に `Error::MissingRequiredElement` を返す）、共有文字列・インラインストリングのリッチテキストラン（`<r><t>...</t></r>` の連結）といった、複数モジュールで重複しがちな小さな共通処理をヘルパー関数として提供する
- **含まない責務**: 個々のXMLパーツ（`_rels` / `workbook.xml` / `sharedStrings.xml` / `styles.xml` / `sheetX.xml`）固有の構造解釈（各サブモジュールの責務）、パース結果の意味的な検証・解決（`resolve/`）

## 主要な型・関数（案）

```rust
mod relationships;
mod workbook;
mod shared_strings;
mod styles;
mod worksheet;
mod theme;

pub(crate) use relationships::{Relationship, RelationshipMap, TargetMode, parse_relationships};
pub(crate) use shared_strings::{SharedStringTable, parse_shared_strings};
pub(crate) use styles::parse_styles;
pub(crate) use workbook::{WorkbookSheetEntry, parse_workbook_xml};
pub(crate) use worksheet::{PendingSharedString, PendingStyle, WorksheetParseOutput, parse_worksheet};
pub(crate) use theme::parse_theme;

use crate::error::Error;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::borrow::Cow;
use std::io::BufRead;

/// XXE対策を適用済みの `Reader` を生成する唯一の窓口。
///
/// `quick-xml` は非検証型パーサーであり、標準構成のままでも外部実体・外部
/// DTDサブセットのフェッチは行わないため、古典的なXXE（ローカルファイル
/// 読み出し・SSRF）はそもそも成立しない。しかし要求仕様書2章の「XMLパース時
/// において外部エンティティの展開を無効化する」という要件を暗黙の前提のまま
/// にせず、設定として明示するために本関数を唯一の生成窓口とする（具体的に
/// 無効化を明示するAPIの有無はcrateのバージョン選定に依存する。オープン
/// クエスチョン1参照）。
///
/// `trim_text(false)` を設定し、要素テキストを自動トリムしない。共有文字列の
/// `xml:space="preserve"`（[shared_strings.md](shared_strings.md) 参照）を
/// 取りこぼさないための既定値であり、空白を保持するかどうかの判断は各
/// サブモジュール側で行う。
pub(crate) fn create_secure_reader<R: BufRead>(inner: R) -> Reader<R> {
    let mut reader = Reader::from_reader(inner);
    reader.config_mut().trim_text(false);
    reader
}

/// `quick_xml::Error` を `crate::error::Error` へ変換する唯一の窓口。
/// [container/sanitize.md エラー処理方針](../container/sanitize.md) が定義する
/// `BoundedReader`（Zip Bomb対策）からの上限超過は `quick_xml::Error::Io` に
/// 包まれた `io::Error` として伝播してくるため、まずこれを
/// `container::sanitize::LimitExceeded` へダウンキャストし、該当すれば
/// `Error::ZipBombDetected` を返す。該当しない場合は `Error::XmlParse` として
/// 型消去して包む（[error.md](../error.md) の方針どおり `quick_xml::Error` を
/// 直接パブリックに晒さない）。
pub(crate) fn convert_xml_error(path: &str, err: quick_xml::Error) -> Error {
    if let quick_xml::Error::Io(io_err) = &err {
        if let Some(limit) = io_err
            .get_ref()
            .and_then(|e| e.downcast_ref::<crate::container::sanitize::LimitExceeded>())
        {
            return Error::ZipBombDetected {
                limit: limit.limit,
                actual: limit.actual,
            };
        }
    }
    Error::XmlParse {
        path: path.to_string(),
        source: Box::new(err),
    }
}

/// イベント読み取りの唯一の窓口。`reader.read_event_into(buf)` を呼び出し
/// `convert_xml_error` でエラー変換したうえで、返された `Event` が
/// `Event::DocType`（`<!DOCTYPE ...>` 宣言）であれば内容を一切解釈せず
/// 無条件に `Error::DoctypeRejected` を返す（fail closed）。
///
/// OOXMLの `_rels`/`workbook.xml`/`sharedStrings.xml`/`styles.xml`/
/// `sheetX.xml` はいずれも仕様上DOCTYPE宣言を持たないため、正当な
/// `.xlsx` をこの検査が誤って拒否することはない。`quick-xml` 自体が
/// DTD内部サブセット・外部実体を解決しない非検証型パーサーであり
/// 標準構成のままでも古典的なXXEは成立しないという設計上の前提
/// （[責務・スコープ](#責務スコープ)参照）はそのまま残るが、本関数は
/// その前提が将来のバージョン変更や別パーサーへの移行によって崩れた
/// 場合にも独立して機能する多層防御として、DOCTYPE宣言の存在自体を
/// XML構文レベルで検知した時点で処理を打ち切る。`parse/` 配下の
/// 各モジュールは本関数経由でのみイベントを読み取り、
/// `reader.read_event_into` を直接呼ばない。
pub(crate) fn read_event<'a>(
    reader: &mut Reader<impl BufRead>,
    buf: &'a mut Vec<u8>,
    path: &str,
) -> Result<Event<'a>, Error> {
    let event = reader
        .read_event_into(buf)
        .map_err(|err| convert_xml_error(path, err))?;
    if matches!(event, Event::DocType(_)) {
        return Err(Error::DoctypeRejected {
            path: path.to_string(),
        });
    }
    Ok(event)
}

/// `start` の属性から `name` を取得する。欠落時は `Error::MissingRequiredElement`
/// を返す。デコード・アンエスケープ済みの文字列を返すか生バイト列のままにするかは
/// quick-xmlのバージョン選定と合わせて確定させる（オープンクエスチョン3参照）。
pub(crate) fn required_attr(
    start: &BytesStart<'_>,
    path: &str,
    name: &'static str,
) -> Result<String, Error> {
    let _ = (start, path, name);
    unimplemented!()
}

/// `<si>`（共有文字列）や `<is>`（インラインストリング）配下のリッチテキスト
/// ラン（`<r><t>...</t></r>` の並び、または単一の `<t>...</t>`）からテキスト
/// のみを連結して取り出す共通ヘルパー。`<rPr>`（ランごとの書式）や
/// `<rPh>`（ふりがな。[shared_strings.md](shared_strings.md) 参照）配下の
/// `<t>` は連結対象に含めない。[shared_strings.md](shared_strings.md) と
/// [worksheet.md](worksheet.md)（`t="inlineStr"` セル）の双方が同一の構造
/// （`<si>`/`<is>` 配下のラン構造はOOXML上共通）を解釈するため、実装の
/// 重複を避けるためにここへ集約する。
///
/// **実装後の改訂(Issue #53/#56/#57)**: 以下は初版実装から大きく変わって
/// いる。結果に寄与するのは `<t>` 要素の**内側**にあるテキストのみ——
/// 初版実装は `<si>`/`<is>` 配下のどこであれ `Event::Text` を無条件に
/// 捕捉しており、「現在 `<t>` の内側にいるか」の判定を持たなかったため、
/// 整形出力(インデント付き)されたXMLで兄弟タグ間の空白が結果へ混入して
/// いた(Issue #53)。各 `<t>` 自身の内容は、`xml:space="preserve"` を
/// 持たない限り先頭・末尾の空白をトリムするようにもなった(Issue #56。
/// Excel・他のExcel互換リーダーの慣習に合わせる——`xml:space` は
/// `<si>`/`<is>` 全体ではなく個々の `<t>` 要素の属性であるため、トリムも
/// ラン単位で行う)。Excelが「XML構文上生の状態では表現できないリテラル
/// なCR」を表すために使う `_x000D_` エスケープは、連結後に復元する
/// (Issue #57)。
///
/// 各フラグメント(`Event::Text`/`Event::CData`/`Event::GeneralRef`)は
/// 一時的なラン単位バッファを経由せず、最終出力の `text` バッファへ
/// 直接追記される。トリムは `<t>` が閉じた時点で `text` 自身の末尾に
/// 対しin-placeで行う(`trim_tail_in_place` 参照)。過渡的な実装では
/// トリムしてから追記するために各ランを専用の `String` へ蓄積していたが、
/// 共有文字列5万件のベンチマークでIssue #56適用前の基準値に対し約17%の
/// 速度低下を招くことが判明した。このin-place方式は追加アロケーションを
/// 完全に回避し(速度低下を約10%まで縮小)、かつ正しくトリムできる——
/// 現在のランのトリムが確定する前に後続のランの内容が追記されることは
/// 決してないため。
///
/// `Event::CData`(`<t><![CDATA[...]]></t>`)も `Event::Text` と同じ方法で
/// デコードするようになった——実際のExcelは書かないが第三者製ツールが
/// 正当に生成しうる形式で、以前は `_ => {}` の受け皿分岐に落ちて
/// サイレントに無視されていた。
///
/// 生の `Event::Text`/`Event::CData` フラグメントはいずれも、`text` へ
/// 追記する前に `normalize_line_endings` を通す: `quick-xml` はXML 1.0
/// §2.11が義務付ける改行正規化(ソース中の生のCRLFまたは単独のCRは、
/// アプリケーションに渡される前に単一のLFへ正規化されなければならない)
/// を実装していないため、本プロジェクト自身のテキスト読み取りパスで
/// 明示的に行う——`_x000D_` を検証する対象のフィクスチャがたまたま生の
/// XMLソース自体もCRLF改行を使っていたため発見に至った。この正規化を
/// 行わないと `\r\r\n` に二重化してしまう。`push_general_ref` の出力には
/// あえて適用しない——明示的な `&#13;` 文字参照は著者が意図的にエンティティ
/// として書き下した本物のCRであり、ソース上の生の改行ではないため、
/// 正規化せずそのまま残す必要がある。
pub(crate) fn concat_rich_text<R: BufRead>(
    reader: &mut Reader<R>,
    path: &str,
) -> Result<String, Error> {
    let mut text = String::new();
    let mut buf = Vec::new();
    let mut skip_depth: u32 = 0; // <rPr>/<rPh> の内側か?
    // `<t>` の内容がtextへ追記され始めたバイトオフセット——<t>の外側では None。
    let mut t_start: Option<usize> = None;
    let mut t_preserve = false;
    loop {
        match read_event(reader, &mut buf, path)? {
            Event::Start(e) if e.local_name().as_ref() == b"rPr" || e.local_name().as_ref() == b"rPh" => skip_depth += 1,
            Event::End(e) if e.local_name().as_ref() == b"rPr" || e.local_name().as_ref() == b"rPh" => skip_depth -= 1,
            Event::Start(e) if skip_depth == 0 && e.local_name().as_ref() == b"t" => {
                t_preserve = optional_attr(&e, path, "xml:space")?.as_deref() == Some("preserve");
                t_start = Some(text.len());
            }
            Event::End(e) if skip_depth == 0 && e.local_name().as_ref() == b"t" => {
                if let Some(start) = t_start.take() {
                    if !t_preserve {
                        trim_tail_in_place(&mut text, start);
                    }
                }
            }
            Event::Text(e) if skip_depth == 0 && t_start.is_some() => {
                let decoded = e.decode().map_err(|err| Error::XmlParse { path: path.to_string(), source: Box::new(err) })?;
                text.push_str(&normalize_line_endings(&decoded));
            }
            Event::CData(e) if skip_depth == 0 && t_start.is_some() => {
                let decoded = e.decode().map_err(|err| Error::XmlParse { path: path.to_string(), source: Box::new(err) })?;
                text.push_str(&normalize_line_endings(&decoded));
            }
            // `&#x...;`/`&#...;`、またはXML定義済み5実体
            // (`&amp;`/`&lt;`/`&gt;`/`&apos;`/`&quot;`)——DOCTYPEを
            // `read_event` が既に拒否しているため、正当に出現しうるのは
            // これらのみ。
            Event::GeneralRef(e) if skip_depth == 0 && t_start.is_some() => {
                push_general_ref(&mut text, &e, path)?
            }
            Event::End(e) if e.local_name().as_ref() == b"si" || e.local_name().as_ref() == b"is" => break,
            Event::Eof => return Err(Error::MissingRequiredElement { path: path.to_string(), name: "si/is closing tag" }),
            _ => {}
        }
        buf.clear();
    }
    // ラン単位ではなく最後にまとめて復元する——マーカーがラン境界を
    // またぐ(非現実的だが)ケースでも正しく動作するように。
    if text.contains("_x000D_") {
        text = text.replace("_x000D_", "\r");
    }
    Ok(text)
}

/// `text[start..]` の先頭・末尾の空白をin-placeでトリムする(追加の
/// アロケーションなし)——1つの `<t>` ランの内容を追記した直後、かつ
/// それ以降のランの内容がまだ追記されていない時点でのみ呼ばれるため、
/// `String::drain` による先頭空白の除去は「このラン自身」の残りバイトを
/// シフトするだけで済み、蓄積済みの文字列全体をシフトすることはない。
fn trim_tail_in_place(text: &mut String, start: usize) {
    let trailing_len = text.len() - start - text[start..].trim_end().len();
    text.truncate(text.len() - trailing_len);
    let leading_len = text[start..].len() - text[start..].trim_start().len();
    if leading_len > 0 {
        text.drain(start..start + leading_len);
    }
}
```

## 依存関係

- 依存先: [`container/sanitize.rs`](../container/sanitize.md)（`LimitExceeded` へのダウンキャストのみ。`container::ZipContainer` そのものには依存しない。architecture.md 設計方針3が禁じるのは「`container` と `parse` が互いのオーケストレーション上の役割を直接知ること」であり、`parse/` が `container::sanitize::LimitExceeded` という1つの内部エラー型のみを参照することはこれに反しない）、[`error.rs`](../error.md)、外部クレート `quick-xml`
- 依存元: `parse/` 配下の全サブモジュール（[relationships.rs](relationships.md) / [workbook.rs](workbook.md) / [shared_strings.rs](shared_strings.md) / [styles.rs](styles.md) / [worksheet.rs](worksheet.md) / [theme.rs](theme.md)）、`pipeline.rs`（再エクスポートされた各パース関数の呼び出し）

`convert_xml_error` が `container::sanitize::LimitExceeded` を参照する設計は、[container/sanitize.md エラー処理方針](../container/sanitize.md)・[container/mod.md エラー処理方針](../container/mod.md) の双方が既に「変換境界は `parse/` が `quick_xml::Error` を `crate::error::Error` へ変換する箇所に置く」と確定させていた内容をそのまま実装したものであり、両ファイルのオープンクエスチョンとして残されていた論点はこれで解決済みとなる。

`read_event` が `Error::DoctypeRejected`（[error.md](../error.md) に新設）を返す設計は、`create_secure_reader`・`convert_xml_error` と並ぶ第3の「唯一の窓口」であり、XXE対策を `Reader` の生成設定という受動的な仕組みだけに委ねず、実際に読み取られる各イベントに対する能動的な検査として二重化する（[セキュリティレビュー](../../security/design-review.md) Finding 1を反映）。

## エラー処理方針

- `create_secure_reader` はエラーを返さない（`Reader` の生成自体は失敗しない。入力ストリームのI/Oエラーは実際に読み取りを行う `read_event` 呼び出し時に顕在化する）
- `convert_xml_error` はあらゆる `quick_xml::Error` を必ず `crate::error::Error` のいずれかのバリアントへ変換する（`panic` しない）。`Error::ZipBombDetected` に該当しない場合のフォールバックは常に `Error::XmlParse` とし、未知のバリアントを握りつぶさない
- `read_event` は `Event::DocType` を検知した場合、宣言の中身（内部サブセットに実体定義を含むか等）を一切解釈せず即座に `Error::DoctypeRejected` を返す。判定を「怪しい実体定義を含む場合のみ拒否する」という許可リスト的な解釈にせず、DOCTYPE宣言の存在そのものを拒否理由とすることで、実体定義の構文解析ミスに起因する検知漏れの余地を構造的に排除する（fail closed。[container/sanitize.md](../container/sanitize.md) の `validate_entry_path` が採る「あいまい・解釈できない入力は安全側に倒す」という方針と同じ考え方）
- `required_attr` は信頼できない外部入力（不正な `.xlsx`）由来の欠落を扱うため `panic` せず `Result` を返す

## テスト方針

- `create_secure_reader` が生成した `Reader` の設定（`trim_text(false)` 等）が期待どおりであることの確認
- `convert_xml_error`: `BoundedReader` が返す `LimitExceeded` を包んだ `io::Error` から生成された `quick_xml::Error::Io` を渡した場合に `Error::ZipBombDetected` へ正しく変換され、`limit`/`actual` の値が保持されることの確認
- `convert_xml_error`: 通常のXML構文エラー（不正なタグの閉じ忘れ等）を渡した場合に `Error::XmlParse` へ変換され、`path` が正しく設定されることの確認
- `required_attr`: 属性が存在する場合に値を取得できること、存在しない場合に `Error::MissingRequiredElement` を返すことの確認
- `concat_rich_text`: 単一の `<t>`、複数の `<r><t>` ラン、および `<rPh>` を含む入力それぞれについて期待どおりの文字列が得られることの確認（詳細な網羅ケースは [shared_strings.md テスト方針](shared_strings.md) 側で行う。本ファイルでは結線の確認に留める）
- **`read_event`: DOCTYPE宣言と外部実体参照を含む悪意あるXML（XXE攻撃ペイロード。例: `<!DOCTYPE foo [ <!ENTITY xxe SYSTEM "file:///etc/passwd"> ]>`）を渡した場合に、`Event::DocType` を検知した時点で `Error::DoctypeRejected` を返し、後続のイベントを一切読み進めないことの確認**（要求仕様書2章のXXE要件そのものの検証。[セキュリティレビュー](../../security/design-review.md) Finding 1で指摘された、暗黙の前提のみに依拠しない明示的・検証可能な対策の回帰テスト）
- `read_event`: 外部実体参照を含まない、DOCTYPE宣言を含まない正当なXML（`_rels`/`workbook.xml`/`sharedStrings.xml`/`styles.xml`/`sheetX.xml` 相当）に対しては通常どおりイベントを返し、`Error::DoctypeRejected` が誤って発生しないことの確認（正当な `.xlsx` に対する偽陽性がないことの回帰テスト）
- `read_event`: DOCTYPE宣言を含まないが構文的に不正なXML、および `BoundedReader` の上限超過を渡した場合に、それぞれ `convert_xml_error` 経由で `Error::XmlParse`/`Error::ZipBombDetected` へ正しく変換されること（`Event::DocType` の判定より前にエラー変換が行われる経路の結線確認）
- 個々の `parse/*.rs` 側では上記のXXE関連テストを重ねて実施しない（[PR #9 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204)を踏まえ、`read_event` が定義されている本ファイルへ集約する）

## 未決事項 / オープンクエスチョン

1. ~~quick-xmlのバージョン選定と `Reader` 設定APIの確定~~ → **解決**: quick-xml 0.41を採用。`Reader::config_mut().trim_text(false)` はドラフト通り。ドラフトからのAPI差分は2点（コンパイルエラー/非推奨警告として顕在化、実装時に修正）:
   - `Attribute::unescape_value()` は非推奨化されており、`Attribute::normalized_value(XmlVersion::Implicit1_0)` を用いる（`required_attr` で使用）。デコード・実体アンエスケープ・AttValue正規化を単一呼び出しで行う点は旧APIと同等
   - `BytesText` は `unescape()` メソッドを持たなくなった。さらに重要な変更として、0.41では `&...;` 参照（文字参照・XML定義済み実体）が周囲の `Event::Text` の生データに埋め込まれなくなり、トークナイザが独立した `Event::GeneralRef(BytesRef)` として分離して出力するようになった。そのため `concat_rich_text`（実体を含むテキストを再構成する唯一の呼び出し元）は両方を処理する: `Event::Text` は `.decode()` のみで済む（アンエスケープすべき実体がもう残っていないため）一方、`Event::GeneralRef` は `BytesRef::resolve_char_ref()`（数値参照）、フォールバックで `quick_xml::escape::resolve_predefined_entity()`（名前付き参照。DOCTYPEを`read_event`が既に拒否しているためカスタム実体は原理上存在せず、正当に出現しうるのはこれらのみ）で解決する。この変更はコンパイルエラーではなく、テスト失敗（`&#x...;` 形式の数値文字参照を使う`rPh`ふりがなのテストケース）で発覚した — `Event::Text` はそのままコンパイル・実行でき、参照先の文字が黙って欠落するだけだったため。

   `read_event` によるXXE対策（`Event::DocType` の無条件拒否）はこれらのAPI変更の影響を受けず、このオープンクエスチョンが想定していた独立性が実装でも確認された（[セキュリティレビュー](../../security/design-review.md) Finding 1を反映）。
2. ~~XXE非該当の実証テストの置き場所~~ → **解決**: 本ファイルのユニットテストへ集約する（[PR #9 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204)を反映）。`read_event` が定義された箇所と同一ファイルに置くことで、対策の核心（`Event::DocType` を確実に検知・拒否すること）を直接検証できる。個々の `parse/*.rs` 側で重ねて実施しない。
3. ~~`required_attr` の返り値の型~~ → **暫定解決**: シンプルさを優先し `String`（アロケーション済み）のまま実装した。`Attribute::normalized_value` 自体は内部で `Cow<str>` を返しており、属性アロケーションがホットパスと判明した場合に見直す。
4. ~~名前空間（`r:id` 等）の解決方式~~ → **解決**: `quick_xml::NsReader` による名前空間URIベースの解決は採用せず、`"r:id"` のようなプレフィックス込みの文字列前方一致（`required_attr` へ渡す属性名にプレフィックスを含めて照合する）で簡略化する（[PR #9 レビュー](https://github.com/MinamiyamaKotaro/xlsxparser/pull/9#pullrequestreview-4948641204)を反映）。Excel・Google スプレッドシート・LibreOffice・Apache POI等、主要な生成ツールがリレーションシップ名前空間のプレフィックスとして例外なく `r` を使用する実務上の慣行を踏まえ、要求仕様書1章が掲げる「軽量かつ高速」という方針を優先する。仮に別名プレフィックスで宣言された正当だが非常に稀なXMLが入力された場合でも、属性が「見つからない」扱いとなり `Error::MissingRequiredElement` として安全側（fail closed）に倒れるため、誤った値を静かに読み取ってしまうリスクはない。
5. **`worksheet.xml` のような大容量ストリームに対する `Reader` の内部バッファサイズ**: quick-xmlはデフォルトでバッファを動的に拡張するが、要求仕様書が想定する「方眼紙Excel」規模のシートに対しては初期バッファサイズを明示的にチューニングする余地がある。[worksheet.md](worksheet.md) の設計・実装時にプロファイリング結果を踏まえて確定させる。
