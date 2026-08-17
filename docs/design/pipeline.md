# `pipeline.rs` 設計書

*[English](pipeline.en.md)*

`src/pipeline.rs` に対応する設計書。[architecture.md](architecture.md) が定義する5フェーズ・パイプラインのオーケストレーターであり、[`container/`](container/mod.md)・[`parse/`](parse/mod.md)・[`resolve/`](resolve/mod.md)・[`model/`](model/mod.md) を呼び出し順序どおりに結線し、リソースの生存期間を制御する。architecture.md 設計方針3「`container` と `parse` の密な往復、およびリソースのライフサイクル管理を `pipeline.rs` に一元化する」を実装する中核ファイル。

## 責務・スコープ

- [`container::ZipContainer`](container/mod.md) を所有し、フェーズ1〜4を通じて `get_entry` を逐次呼び出す（1エントリを読み切ってから次のエントリを取得する。[container/mod.md](container/mod.md) が `get_entry` の型シグネチャで既に強制している逐次アクセスパターンに従う）
- **フェーズ1**: `xl/_rels/workbook.xml.rels` と `xl/workbook.xml` を取得・パースし、シート名・可視性・実体ファイルパスの「ルーティングプラン」を構築する。あわせて `xl/_rels/workbook.xml.rels` 内から `sharedStrings.xml` / `styles.xml` への関係を関係タイプ（`Relationship.rel_type`）で識別する（[relationships.md 含まない責務](parse/relationships.md) が「どの r:id がどのパーツ種別に対応するかの意味づけは呼び出し元の責務」としていた分担を実装する）
- ルーティングプラン構築後、rels読み込みに使ったリーダーと [`parse::RelationshipMap`](parse/relationships.md) をスコープアウトさせ破棄する（architecture.md「フェーズ1完了時にルーティングマップ構築後、`_rels` の一時バッファを破棄する」の実装）
- ルーティングプラン確定後、シートループに入る前に [`SharedStringTable`](parse/shared_strings.md) と [`StyleSheet`](model/style.md) を一度だけ構築する
- シートごとに [`model::Sheet::new`](model/sheet.md) で空シートを構築し、対応するエントリを [`parse::parse_worksheet`](parse/worksheet.md) に渡してストリームでセルを挿入させ（フェーズ3）、その出力を [`resolve::resolve_sheet`](resolve/mod.md) へ渡して解決する（フェーズ4）
- 全シートの処理完了後、[`SharedStringTable`](parse/shared_strings.md) と [`StyleSheet`](model/style.md) をスコープアウトさせ破棄し、[`model::Workbook::new`](model/workbook.md) で最終モデルを構築して返す
- **含まない責務**: 各フェーズそのもののロジック（ZIP展開・サニタイズは `container/`、XML構造の解釈は `parse/`、意味解決は `resolve/`）、行単位のXMLノード破棄（フェーズ3の内部詳細であり `parse/worksheet.rs` が担う。architecture.md「`pipeline.rs` はこれを制御しない」）、JSON生成そのもの（[`json.rs`](json.md)。呼び出すかどうかの設計上の位置づけはオープンクエスチョン1参照）

## 主要な型・関数（案）

```rust
use crate::container::ZipContainer;
use crate::error::Error;
use crate::model::sheet::{Sheet, SheetVisibility};
use crate::model::workbook::Workbook;
use crate::parse::shared_strings::SharedStringTable;
use crate::{container, model, parse, resolve};
use std::io::{Read, Seek};

const WORKBOOK_RELS_PATH: &str = "xl/_rels/workbook.xml.rels";
const WORKBOOK_PATH: &str = "xl/workbook.xml";
const SHARED_STRINGS_REL_TYPE_SUFFIX: &str = "/relationships/sharedStrings";
const STYLES_REL_TYPE_SUFFIX: &str = "/relationships/styles";

/// 1シート分のルーティング情報。フェーズ1完了時点で確定する。
struct SheetRoute {
    name: String,
    visibility: SheetVisibility,
    /// container::get_entry にそのまま渡せる、ZIPエントリ名相当の絶対パス。
    worksheet_path: String,
}

/// フェーズ1〜5全体を実行し、解決済みの `Workbook` を返す。`lib.rs` の
/// 公開API（`parse_workbook` 等。オープンクエスチョン2参照）が本関数を呼ぶ。
/// `Read + Seek` に対して汎用的なのは [container/mod.md](container/mod.md) の
/// `ZipContainer::open_reader` の制約（ZIP central directory 読み取りに
/// シーク可能性を要求する）をそのまま引き継ぐため。
pub(crate) fn run<R: Read + Seek>(reader: R) -> Result<Workbook, Error> {
    let mut container = ZipContainer::open_reader(reader)?;

    // --- フェーズ1: リレーションシップの解決とルーティングプラン構築 ---
    let rels_reader = container
        .get_entry(WORKBOOK_RELS_PATH)?
        .ok_or_else(|| Error::MissingRelationshipPart(WORKBOOK_RELS_PATH.to_string()))?;
    let relationships = parse::parse_relationships(rels_reader, "xl", WORKBOOK_RELS_PATH)?;

    let workbook_reader = container
        .get_entry(WORKBOOK_PATH)?
        .ok_or_else(|| Error::InvalidPackage(WORKBOOK_PATH.to_string()))?;
    let sheet_entries = parse::parse_workbook_xml(workbook_reader, WORKBOOK_PATH)?;

    let mut routes = Vec::with_capacity(sheet_entries.len());
    for entry in sheet_entries {
        let rel = relationships
            .get(&entry.r_id)
            .ok_or_else(|| Error::DanglingRelationship { r_id: entry.r_id.clone() })?;
        routes.push(SheetRoute {
            name: entry.name,
            visibility: entry.visibility,
            worksheet_path: rel.target.clone(),
        });
    }
    let shared_strings_path = relationships
        .values()
        .find(|r| r.rel_type.ends_with(SHARED_STRINGS_REL_TYPE_SUFFIX))
        .map(|r| r.target.clone());
    // styles.xml は OOXML上必須パーツのため見つからない場合は Error::InvalidPackage とする。
    let styles_path = relationships
        .values()
        .find(|r| r.rel_type.ends_with(STYLES_REL_TYPE_SUFFIX))
        .map(|r| r.target.clone())
        .ok_or_else(|| Error::InvalidPackage("styles relationship not found".to_string()))?;

    // rels読み込みに使ったリーダー・RelationshipMapはここでスコープアウトし破棄される
    // （architecture.md「フェーズ1完了時に_relsの一時バッファを破棄する」の実装）。
    drop(relationships);

    // --- フェーズ1〜3の間で一度だけ構築される共有テーブル ---
    let shared_string_table = match shared_strings_path {
        Some(path) => {
            let reader = container
                .get_entry(&path)?
                .ok_or_else(|| Error::InvalidPackage(path.clone()))?;
            parse::parse_shared_strings(reader, &path)?
        }
        // sharedStrings.xml 自体はOOXML上の任意パーツ（文字列セルを1つも
        // 持たないブックでは省略されうる）。
        None => SharedStringTable::default(),
    };
    let styles_reader = container
        .get_entry(&styles_path)?
        .ok_or_else(|| Error::InvalidPackage(styles_path.clone()))?;
    let stylesheet = parse::parse_styles(styles_reader, &styles_path)?;

    // --- シートごとにフェーズ3（ストリームパース）→フェーズ4（解決）---
    let mut sheets = Vec::with_capacity(routes.len());
    for route in routes {
        let mut sheet = Sheet::new(route.name, route.visibility);
        let reader = container.get_entry(&route.worksheet_path)?.ok_or_else(|| {
            Error::DanglingRelationship { r_id: route.worksheet_path.clone() }
        })?;
        let output = parse::parse_worksheet(reader, &route.worksheet_path, &mut sheet)?;
        resolve::resolve_sheet(
            &mut sheet,
            &output.pending_shared_strings,
            &shared_string_table,
            &output.pending_styles,
            &stylesheet,
            output.merge_regions,
        )?;
        sheets.push(sheet);
    }
    // shared_string_table / stylesheet はここでスコープアウトし破棄される
    // （architecture.md「フェーズ4完了時にSharedStringTableやStyleSheetを
    // 破棄する」の実装）。

    Ok(Workbook::new(sheets))
}
```

## 依存関係

- 依存先: [`container/mod.rs`](container/mod.md)（`ZipContainer`）、[`parse/mod.rs`](parse/mod.md)（`parse_relationships`, `parse_workbook_xml`, `parse_shared_strings`, `parse_styles`, `parse_worksheet`, `SharedStringTable`）、[`resolve/mod.rs`](resolve/mod.md)（`resolve_sheet`）、[`model/sheet.rs`](model/sheet.md)（`Sheet::new`, `SheetVisibility`）、[`model/workbook.rs`](model/workbook.md)（`Workbook::new`）、[`error.rs`](error.md)
- 依存元: `lib.rs`（公開APIから呼び出される。オープンクエスチョン2参照）

`run` の実装が1エントリずつ確実に処理を終えてから次の `get_entry` を呼ぶ構造になっているのは偶然ではない。[container/mod.md](container/mod.md) の `get_entry` は `&mut self` を要求し返り値の生存期間を `self` の借用へ束縛する設計（`impl Read + '_`）のため、複数のエントリを同時に開いたまま処理することはRustの借用チェッカーによりコンパイル時に禁止される。`pipeline.rs` の逐次的な制御フローは、この型制約と自然に一致する形で導出される（[container/mod.md オープンクエスチョン2の解決](container/mod.md)が想定していた「rels読み込み→SST読み込み→worksheet逐次読み込み」というアクセスパターンそのもの）。

## エラー処理方針

- 各フェーズの失敗を `?` で早期リターンし、後続フェーズを実行しない（[resolve/mod.md](resolve/mod.md) の `resolve_sheet` と同じ fail closed の原則）。1シートでもパース・解決に失敗した場合、それまでに処理済みの他シートを含めて `Workbook` を返さない（部分的に壊れたブックを黙って返さない。オープンクエスチョン4参照）
- `container::get_entry` が返す `Ok(None)`（エントリ不在）からどの `Error` バリアントを構築するかは、[container/mod.md](container/mod.md) が「呼び出し側の文脈でしか判断できない」としていたとおり本ファイルの責務とする:
  - `xl/_rels/workbook.xml.rels` 不在 → `Error::MissingRelationshipPart`（フェーズ1の必須パーツ）
  - `xl/workbook.xml` 不在、または rels が指す `styles.xml` の実体パーツが不在 → `Error::InvalidPackage`（OPCパッケージとして必須のパーツを欠く）
  - `workbook.xml` の `<sheet r:id="...">` が指す r:id が `RelationshipMap` に存在しない、または rels が指す worksheet の実体パーツが不在 → `Error::DanglingRelationship`
- `sharedStrings.xml` に対応する関係が見つからない場合はエラーにせず `SharedStringTable::default()`（空テーブル）にフォールバックする（OOXML上の任意パーツであるため。`styles.xml` は必須パーツのため同様のフォールバックは行わない）

## テスト方針

- 正当な最小構成の `.xlsx` 相当ZIP（1シート、数値・共有文字列参照・結合セルを含む）を `run` に渡し、`Ok` で期待する `Workbook` が得られることの確認（統合テスト）
- `xl/_rels/workbook.xml.rels` が存在しないZIPに対し `Error::MissingRelationshipPart` を返すことの確認
- `xl/workbook.xml` が存在しないZIPに対し `Error::InvalidPackage` を返すことの確認
- `workbook.xml` の `<sheet r:id="...">` が指す r:id が rels 内に存在しない場合に `Error::DanglingRelationship` を返すことの確認
- rels 内の styles 関係が指す実体ファイルがZIP内に存在しない場合に `Error::InvalidPackage` を返すことの確認
- rels 内に worksheet 関係の実体ファイルが存在しない場合に `Error::DanglingRelationship` を返すことの確認
- `sharedStrings.xml` パーツ自体が存在しない（文字列セルを一切含まない）ブックでもエラーにならず、空の `SharedStringTable` で正常に完走することの確認
- 複数シートを持つブックで、各シートが `xl/workbook.xml` の `<sheets>` 定義順で `Workbook.sheets()` に格納されることの確認（[model/workbook.md](model/workbook.md) のソース順維持方針との結線）
- 途中のシート（例: 2枚目）のパースが失敗する場合に、1枚目が正常に処理済みであっても `Workbook` 全体が返らず `Err` になることの確認（fail closed の回帰テスト）
- 可視性が `Hidden`/`VeryHidden` のシートを含むブックでも、全シートが除外されずに `Workbook` へ含まれることの確認（[model/workbook.md オープンクエスチョン1](model/workbook.md) との結線）

## 未決事項 / オープンクエスチョン

1. **`json.rs` を `run` 内から呼ぶかどうか**: [architecture.md](architecture.md) の `pipeline.rs` 節は「`resolve` で解決した結果を `json.rs` でシリアライズする」と5フェーズ全体の流れを説明する一方、[model/workbook.md](model/workbook.md) は既に「`lib.rs` の公開API（`parse_workbook(path) -> Result<Workbook>`）の返り値そのものになる」と明記しており、`Workbook`（JSONではなく構造化データ）が主要な返り値であることを前提としている。本設計はこの矛盾を、architecture.md の記述を「クレート全体が提供する5フェーズの機能」を示す概念的な説明と解釈し、`run` 自体はフェーズ1〜4（`Workbook` を返す）までを担い、フェーズ5（JSON化）は [`json.rs`](json.md) が提供する別関数として `Workbook` から明示的に呼び出す2段構成と解決した。`lib.rs`（未設計）がこの2段構成をどう公開するか（`parse_workbook` と `parse_workbook_json` を別々に公開する、`Workbook` に `to_json` メソッドを生やす等）は `lib.rs` の設計時に確定させる。
2. **`lib.rs` との結線**: `run` は `pub(crate)` とし、パス文字列から `std::fs::File` を開いて渡す薄いラッパーを `lib.rs` 側の公開関数として想定しているが、`Read + Seek` を実装する任意の入力（インメモリバッファ等）をどこまで `lib.rs` の公開APIとして許容するかは `lib.rs` の設計時に確定させる。
3. **`[Content_Types].xml` の検証要否**: 現状 `[Content_Types].xml` の中身を一切参照せず、`xl/workbook.xml` や `xl/_rels/workbook.xml.rels` といった固定パスへ直接アクセスしている。実務上のExcel生成ファイルはこれらのパスが事実上固定的だが、厳密なOPC準拠のためには `[Content_Types].xml` のContent-Type宣言を介してパーツを解決すべきという意見もありうる。
4. **個々のシートのパース失敗時の耐性**: 現状は1シートでもエラーがあれば `run` 全体を `Err` で返す設計（fail closed）を採用している。要求仕様書に「壊れたシートをスキップして他シートだけ返す」という要件はないためこの設計としたが、将来的にエラー耐性モード（部分的に読めたデータを返す）の要求が生じた場合は見直しが必要になる。
5. **並行処理**: 現状シートを1つずつ逐次処理する設計（依存関係セクションで述べたとおり `container::get_entry` の逐次アクセス制約とも自然に合致する）。要求仕様書に並列化要件はないが、複数シートを持つ大規模ブックに対するパフォーマンス最適化として、各シートのバイト列を先にすべてメモリへ読み出したうえでスレッドプール等により並列処理する余地はあるか（この場合ストリーミング方針とはトレードオフになる）は、実装後のプロファイリング結果を踏まえて再検討する。
6. ~~`BufRead` 要求と `container::get_entry` の返り値の型の不整合~~ → **実装時に解決**: `parse::parse_*` の各関数はいずれも `impl BufRead` を要求する（quick-xmlの `Reader::read_event_into` がこれを必要とするため）が、`container::get_entry` が返す `BoundedReader<'_, impl Read + '_>` は `Read` のみを実装する。`run` は `get_entry` から得た各readerを `parse::parse_*` へ渡す前に `std::io::BufReader::new(..)` でラップする。上記コードブロックのドラフトはこの点を反映していないが、これは未決の設計論点だったからではなく、`container/` と `parse/` がそれぞれ独立に設計されており、両者を実際に結合する `pipeline.rs` のコンパイルまでこの境界面が検証されていなかったための単純な抜けである。
