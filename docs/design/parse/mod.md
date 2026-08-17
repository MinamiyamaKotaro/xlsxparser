# `parse/mod.rs` 設計書

*[English](mod.en.md)*

`src/parse/mod.rs` に対応する設計書。[architecture.md](../architecture.md) が定義する `parse/` の責務「`quick-xml` などXMLパースライブラリへの依存を集約する層」のうち、個々のパーサー（[relationships.md](relationships.md) / [workbook.md](workbook.md) / [shared_strings.md](shared_strings.md) / [styles.md](styles.md) / [worksheet.md](worksheet.md)）に共通する横断的関心事（セキュアな `Reader` 生成、XMLパースエラーの変換、属性取得・リッチテキスト結合の共通ヘルパー）を集約する。[container/sanitize.md エラー処理方針](../container/sanitize.md) が「`parse/mod.rs` のセキュアReaderファクトリに併設予定」としていた変換ロジックの実体（`convert_xml_error`）はこのファイルで確定させる。

## 責務・スコープ

- サブモジュールの宣言（`mod relationships; mod workbook; mod shared_strings; mod styles; mod worksheet;`）とクレート内公開型の再エクスポート
- XXE対策を適用済みの `quick_xml::Reader` を生成する唯一の窓口 `create_secure_reader` を提供する。`parse/` 配下の各モジュールはこの関数経由でのみ `Reader` を取得しなければならず、個別に `Reader::from_reader` を呼ばない（architecture.md 「各パーサーが個別に `Reader` を初期化すると設定漏れのリスクがあるため」を実装する）
- `quick_xml::Error` を [`crate::error::Error`](../error.md) へ変換する唯一の窓口 `convert_xml_error` を提供する。[container/sanitize.md](../container/sanitize.md) が定義する `BoundedReader` からの上限超過（Zip Bomb）を検知し `Error::ZipBombDetected` へ変換する処理もここに集約する
- 必須属性の取得（欠落時に `Error::MissingRequiredElement` を返す）、共有文字列・インラインストリングのリッチテキストラン（`<r><t>...</t></r>` の連結）といった、複数モジュールで重複しがちな小さな共通処理をヘルパー関数として提供する
- **含まない責務**: 個々のXMLパーツ（`_rels` / `workbook.xml` / `sharedStrings.xml` / `styles.xml` / `sheetX.xml`）固有の構造解釈（各サブモジュールの責務）、パース結果の意味的な検証・解決（`resolve/`）

## 主要な型・関数（案）

```rust
mod relationships;
mod workbook;
mod shared_strings;
mod styles;
mod worksheet;

pub(crate) use relationships::{Relationship, RelationshipMap, TargetMode, parse_relationships};
pub(crate) use shared_strings::{SharedStringTable, parse_shared_strings};
pub(crate) use styles::parse_styles;
pub(crate) use workbook::{WorkbookSheetEntry, parse_workbook_xml};
pub(crate) use worksheet::{WorksheetParseOutput, parse_worksheet};

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
pub(crate) fn concat_rich_text<R: BufRead>(
    reader: &mut Reader<R>,
    buf: &mut Vec<u8>,
) -> Result<String, Error> {
    let _ = (reader, buf);
    unimplemented!()
}
```

## 依存関係

- 依存先: [`container/sanitize.rs`](../container/sanitize.md)（`LimitExceeded` へのダウンキャストのみ。`container::ZipContainer` そのものには依存しない。architecture.md 設計方針3が禁じるのは「`container` と `parse` が互いのオーケストレーション上の役割を直接知ること」であり、`parse/` が `container::sanitize::LimitExceeded` という1つの内部エラー型のみを参照することはこれに反しない）、[`error.rs`](../error.md)、外部クレート `quick-xml`
- 依存元: `parse/` 配下の全サブモジュール（[relationships.rs](relationships.md) / [workbook.rs](workbook.md) / [shared_strings.rs](shared_strings.md) / [styles.rs](styles.md) / [worksheet.rs](worksheet.md)）、`pipeline.rs`（再エクスポートされた各パース関数の呼び出し）

`convert_xml_error` が `container::sanitize::LimitExceeded` を参照する設計は、[container/sanitize.md エラー処理方針](../container/sanitize.md)・[container/mod.md エラー処理方針](../container/mod.md) の双方が既に「変換境界は `parse/` が `quick_xml::Error` を `crate::error::Error` へ変換する箇所に置く」と確定させていた内容をそのまま実装したものであり、両ファイルのオープンクエスチョンとして残されていた論点はこれで解決済みとなる。

## エラー処理方針

- `create_secure_reader` はエラーを返さない（`Reader` の生成自体は失敗しない。入力ストリームのI/Oエラーは実際に読み取りを行う `read_event` 呼び出し時に顕在化する）
- `convert_xml_error` はあらゆる `quick_xml::Error` を必ず `crate::error::Error` のいずれかのバリアントへ変換する（`panic` しない）。`Error::ZipBombDetected` に該当しない場合のフォールバックは常に `Error::XmlParse` とし、未知のバリアントを握りつぶさない
- `required_attr` は信頼できない外部入力（不正な `.xlsx`）由来の欠落を扱うため `panic` せず `Result` を返す

## テスト方針

- `create_secure_reader` が生成した `Reader` の設定（`trim_text(false)` 等）が期待どおりであることの確認
- `convert_xml_error`: `BoundedReader` が返す `LimitExceeded` を包んだ `io::Error` から生成された `quick_xml::Error::Io` を渡した場合に `Error::ZipBombDetected` へ正しく変換され、`limit`/`actual` の値が保持されることの確認
- `convert_xml_error`: 通常のXML構文エラー（不正なタグの閉じ忘れ等）を渡した場合に `Error::XmlParse` へ変換され、`path` が正しく設定されることの確認
- `required_attr`: 属性が存在する場合に値を取得できること、存在しない場合に `Error::MissingRequiredElement` を返すことの確認
- `concat_rich_text`: 単一の `<t>`、複数の `<r><t>` ラン、および `<rPh>` を含む入力それぞれについて期待どおりの文字列が得られることの確認（詳細な網羅ケースは [shared_strings.md テスト方針](shared_strings.md) 側で行う。本ファイルでは結線の確認に留める）
- DOCTYPE宣言と外部実体参照を含む悪意あるXML（XXE攻撃ペイロード）を `create_secure_reader` 経由でパースさせた場合に、外部ファイルの内容がパース結果に一切現れないことを確認する統合的なテスト（要求仕様書2章のXXE要件そのものの検証。個々の `parse/*.rs` 側でも実施するか本ファイルに集約するかは未決定、オープンクエスチョン2参照）

## 未決事項 / オープンクエスチョン

1. **quick-xmlのバージョン選定と `Reader` 設定APIの確定**: [error.md オープンクエスチョン1](../error.md)・[container/mod.md オープンクエスチョン1](../container/mod.md) と連動する論点。バージョンによって `Reader::config_mut()` の有無や設定項目名が異なるため、`Cargo.toml` 整備時に本ファイルのコード例を実際のAPIに合わせて更新する。
2. **XXE非該当の実証テストの置き場所**: 本ファイルに集約するか、個々の `parse/*.rs`（特に外部入力を最初に受け取る [relationships.rs](relationships.md)）側でも重ねて実施するかは未確定。
3. **`required_attr` の返り値の型**: `String`（アンエスケープ・アロケーション済み）ではなく `Cow<str>` や `&str` を返すことで不要なアロケーションを避けられる可能性があるが、quick-xmlの属性デコードAPI（バージョン依存）と合わせて確定させる。
4. **名前空間（`r:id` 等）の解決方式**: `workbook.xml` の `<sheet r:id="...">`、`worksheet.xml` セルの一部属性など、OOXMLの `r` 名前空間プレフィックスに依存する属性照合が [workbook.md](workbook.md) を含む複数モジュールに現れる。プレフィックス `r` は慣例的にほぼ固定されるが、技術的には `xmlns:foo="http://schemas.openxmlformats.org/officeDocument/2006/relationships"` のように別名で宣言される正当なXMLも存在するため、文字列前方一致ではなく名前空間URIベースで解決する `quick_xml::NsReader` を `create_secure_reader` の返り値として採用すべきかは未確定。採用する場合は `parse/` 配下全体のAPIに影響するため、本ファイルで一括して決定する必要がある。
5. **`worksheet.xml` のような大容量ストリームに対する `Reader` の内部バッファサイズ**: quick-xmlはデフォルトでバッファを動的に拡張するが、要求仕様書が想定する「方眼紙Excel」規模のシートに対しては初期バッファサイズを明示的にチューニングする余地がある。[worksheet.md](worksheet.md) の設計・実装時にプロファイリング結果を踏まえて確定させる。
