# `parse/shared_strings.rs` 設計書

*[English](shared_strings.en.md)*

`src/parse/shared_strings.rs` に対応する設計書。[architecture.md](../architecture.md) が定義する `parse/` の責務のうち「`sharedStrings.xml` のパース（SSTの構造化データ抽出）」を担う。`xl/sharedStrings.xml` をパースし、[resolve/shared_strings.md](../resolve/shared_strings.md) が消費する `SharedStringTable` を構築する。同ファイルの[オープンクエスチョン1](../resolve/shared_strings.md)（「`SharedStringTable` の型・配置場所は `parse/shared_strings.rs` の設計時に確定させる」）を解決するファイルでもある。

## 責務・スコープ

- `xl/sharedStrings.xml` の `<sst><si>...</si>...</sst>` をパースし、`<si>` のソース順（＝共有文字列インデックス順）で文字列を保持する `SharedStringTable` を構築する
- 各 `<si>` について、単純な `<t>...</t>` 形式・リッチテキストラン `<r><t>...</t></r>...` 形式のいずれであってもテキストのみを連結した1つの `String` として解決する（書式情報 `<rPr>` は破棄する。[model/cell.md](../model/cell.md) の `CellValue::Text` がプレーンな文字列のみを保持する設計と一致させる）
- `<rPh>`（ふりがな注記。日本語の氏名・地名等でExcelが自動生成することがある）配下の `<t>` を本文と誤って連結しないようにする
- `xml:space="preserve"` を持つ `<t>` の前後空白を保持する（要求仕様書4章 「`xml:space="preserve"` を遵守すること」の実装）
- **含まない責務**: `t="s"` セルのインデックスから実文字列への解決そのもの（[`resolve/shared_strings.rs`](../resolve/shared_strings.md)。本ファイルは解決に使われるテーブルを構築するのみ）、数式文字列（`t="str"`）・インラインストリング（`t="inlineStr"`）の解決（[`parse/worksheet.rs`](worksheet.md) がストリーム中に直接処理する。[resolve/shared_strings.md](../resolve/shared_strings.md) 参照）

## 主要な型・関数（案）

```rust
use crate::error::Error;
use crate::parse::{concat_rich_text, create_secure_reader, read_event};
use quick_xml::events::Event;
use std::io::BufRead;
use std::sync::Arc;

/// 共有文字列テーブル。[resolve/shared_strings.rs](../resolve/shared_strings.md)
/// が `t="s"` セルのインデックス解決に用いる（同ファイルのオープンクエスチョン1を解決）。
#[derive(Debug, Clone, Default)]
pub(crate) struct SharedStringTable {
    strings: Vec<Arc<str>>,
}

impl SharedStringTable {
    /// インデックスから実文字列を引く。`resolve/shared_strings.rs::resolve`
    /// が範囲外時に `Error::SharedStringIndexOutOfBounds` を構築する際に使う。
    pub(crate) fn get(&self, index: usize) -> Option<&Arc<str>> {
        self.strings.get(index)
    }

    pub(crate) fn len(&self) -> usize {
        self.strings.len()
    }
}

/// `xl/sharedStrings.xml` をパースし、`SharedStringTable` を構築する。
/// `<sst>` 直下の各 `<si>` は `concat_rich_text` により1つの文字列へ
/// 解決される（単純な `<t>`・複数ランのリッチテキスト・空の `<si/>` の
/// いずれも扱う）。
pub(crate) fn parse_shared_strings(reader: impl BufRead, path: &str) -> Result<SharedStringTable, Error> {
    let mut xml_reader = create_secure_reader(reader);
    let mut buf = Vec::new();
    let mut strings = Vec::new();
    loop {
        let event = read_event(&mut xml_reader, &mut buf, path)?;
        match &event {
            Event::Start(e) if e.local_name().as_ref() == b"si" => {
                buf.clear();
                let text = concat_rich_text(&mut xml_reader, path)?;
                strings.push(Arc::from(text));
                continue;
            }
            Event::Empty(e) if e.local_name().as_ref() == b"si" => {
                strings.push(Arc::from(""));
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(SharedStringTable { strings })
}
```

## 依存関係

- 依存先: [`parse/mod.rs`](mod.md)（`create_secure_reader`, `read_event`, `concat_rich_text`）、[`error.rs`](../error.md)。`model/` には依存しない（`SharedStringTable` は `parse/` 固有の中間データ構造であり、`model::CellValue::Text` へは [`resolve/shared_strings.rs`](../resolve/shared_strings.md) が変換する）
- 依存元: [`resolve/shared_strings.rs`](../resolve/shared_strings.md)（`SharedStringTable::get`）、[`resolve/mod.rs`](../resolve/mod.md)（`resolve_sheet` の引数として受け渡す）、`pipeline.rs`（フェーズ1〜3の間で一度だけ構築し、`resolve_sheet` の呼び出しに渡す。architecture.md 「フェーズ4完了時に `SharedStringTable` や `StyleSheet` を破棄する」に従い、全シートの解決が終わった時点で破棄する）

`resolve/shared_strings.rs` が `SharedStringTable` を（`model/` を経由せず）直接 `parse::shared_strings` から `use` する設計は、[resolve/mod.md 依存関係](../resolve/mod.md) が既に「`parse::shared_strings::SharedStringTable` への依存はarchitecture.md設計方針2が禁じる『I/Oへの依存』ではなく『フェーズ3が既に構築済みの、メモリ上の構造化データへの依存』である」と整理していた前提をそのまま実装する。

## エラー処理方針

- XMLとして構文的に不正な場合は [`convert_xml_error`](mod.md) を通じて `Error::XmlParse` または `Error::ZipBombDetected` に変換する
- `<si>` の構造自体（`<t>`/`<r>` のいずれも持たない、テキストを持たない空の `<si/>`）は空文字列として扱い、エラーにしない（Excelが空セルの共有文字列エントリを生成することは実際にありうる）
- 個々の `<si>` の解釈に失敗した場合でも `panic` しない（信頼できない外部入力のため）

## テスト方針

- 単純な `<si><t>...</t></si>` の複数件が正しい順序（ソース順＝インデックス順）で `SharedStringTable` に格納されることの確認
- リッチテキスト（複数の `<r><t>...</t></r>`）を持つ `<si>` が、各ランのテキストを結合した1つの文字列として解決されることの確認
- `<rPh>`（ふりがな）要素配下の `<t>` が本文へ誤って連結されないことの確認（日本語業務システム特有の回帰テスト観点。要求仕様書1章が主眼とする利用文脈に対応）
- 先頭・末尾に空白を含む `<t xml:space="preserve"> ... </t>` の空白がトリムされず保持されることの確認
- 空の `<si/>`（テキストを持たない）が空文字列として扱われ、パース全体を失敗させないことの確認
- 空の `<sst>`（`uniqueCount="0"`）に対し空の `SharedStringTable`（`len() == 0`）を返すことの確認
- `SharedStringTable::get` が範囲外インデックスに対し `None` を返すことの確認（`resolve/shared_strings.rs` 側のエラー変換の前提となる結線テスト）

## 未決事項 / オープンクエスチョン

1. ~~`xml:space` 属性値そのものを分岐せず常に非トリムとする設計の妥当性~~ — **解決済み（Issue #56）。** [`parse/mod.rs`](mod.md) の `create_secure_reader` は今も `trim_text(false)` を既定とするが（quick-xml自体は空白のみのテキストノードを無意味なものとして扱わない）、`concat_rich_text` 自身が `<t>` 要素ごとに分岐するようになった——その `<t>` が `xml:space="preserve"` を持たない限り、各ランの前後空白をin-placeでトリムする（Excel自身の慣習に合わせる）。実装は [parse/mod.md](mod.md) の `concat_rich_text`/`trim_tail_in_place` を参照。
2. **大規模な `sharedStrings.xml`（`uniqueCount` が非常に大きい方眼紙Excel等）に対するメモリ確保戦略**: `<sst count="N" uniqueCount="M">` の `uniqueCount` 属性を先読みし `Vec::with_capacity(M)` による事前確保を行うべきかは、パフォーマンス要件と合わせて確定させる。
3. **名前空間の扱い**: [parse/mod.md オープンクエスチョン4](mod.md) と同一の論点。`<t>` / `<r>` / `<rPh>` 自体は接頭辞を持たない要素のため、本ファイルへの影響は限定的と見込む。
