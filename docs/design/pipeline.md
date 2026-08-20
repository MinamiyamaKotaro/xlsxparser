# `pipeline.rs` 設計書

*[English](pipeline.en.md)*

`src/pipeline.rs` に対応する設計書。[architecture.md](architecture.md) が定義する5フェーズ・パイプラインのオーケストレーターであり、[`container/`](container/mod.md)・[`parse/`](parse/mod.md)・[`resolve/`](resolve/mod.md)・[`model/`](model/mod.md) を呼び出し順序どおりに結線し、リソースの生存期間を制御する。architecture.md 設計方針3「`container` と `parse` の密な往復、およびリソースのライフサイクル管理を `pipeline.rs` に一元化する」を実装する中核ファイル。

## 責務・スコープ

- [`container::ZipContainer`](container/mod.md) を所有し、フェーズ1〜4を通じて `get_entry` を逐次呼び出す（1エントリを読み切ってから次のエントリを取得する。[container/mod.md](container/mod.md) が `get_entry` の型シグネチャで既に強制している逐次アクセスパターンに従う）
- 呼び出し元（`lib.rs`）から受け取った `SizeLimits`（[lib.md](lib.md)）を `ZipContainer::open_reader` 直後に `with_max_entry_size` / `with_max_total_size`（[container/mod.md](container/mod.md)）へ橋渡しし、Zip Bombサイズ上限を呼び出し側が上書きできるようにする（セキュリティレビュー Finding 2、Issue [#14](https://github.com/MinamiyamaKotaro/xlsxparser/issues/14)）。同じ `SizeLimits` の `max_cells_per_sheet` は、シートごとの `parse::parse_worksheet` 呼び出し（フェーズ3）へそのまま橋渡しする（Issue [#88](https://github.com/MinamiyamaKotaro/xlsxparser/issues/88)。[container/sanitize.md](container/sanitize.md) 参照）
- **フェーズ1**: まずパッケージルート自身の関係パーツ `_rels/.rels`(OPC仕様でパスが固定される、`xl/_rels/workbook.xml.rels`とは異なる唯一の例外)を取得・パースし、`officeDocument` 関係タイプが指すターゲットをworkbookパーツの実際のパスとして解決する(Issue [#55](https://github.com/MinamiyamaKotaro/xlsxparser/issues/55))。以前は `xl/workbook.xml` を固定パスとして直接読みに行っていたが、OPC上この固定は仕様上の保証ではなく、`_rels/.rels` 経由でのみ発見可能なworkbookパーツを持つ実在ファイル(`tests/fixtures/other/minimal_package.xlsx`、calamineテストコーパス由来)が確認されたため、`_rels/.rels`→`officeDocument`関係→workbookパーツという本来の解決順に改めた。workbookパーツ自身の `_rels`(`xl/_rels/workbook.xml.rels` 相当)のパスも、この解決済みworkbookパーツパスから `rels_path_for`(後述「主要な型・関数（案）」参照)で導出する——`xl/workbook.xml` という固定文字列にはもう依存しない。続けてそのworkbookパーツの `_rels` と `workbook.xml` 本体を取得・パースし、シート名・可視性・実体ファイルパスの「ルーティングプラン」を構築する。あわせてworkbookパーツの `_rels` 内から `sharedStrings.xml` / `styles.xml` への関係を関係タイプ（`Relationship.rel_type`）で識別する（[relationships.md 含まない責務](parse/relationships.md) が「どの r:id がどのパーツ種別に対応するかの意味づけは呼び出し元の責務」としていた分担を実装する）。いずれの関係も存在しない場合がありうる(`sharedStrings.xml` は以前から任意パーツ扱い、`styles.xml` もIssue #54でこれに合流した——詳細はエラー処理方針参照)。本フェーズでは `ParsedWorkbookXml::date1904`(Issue #40)も読み取り、ローカル変数として保持する——`Workbook` のフィールドには決してならず、`StyleSheet`(後述)と同じ「フェーズ間の一時値」として扱う。以前はフェーズ4の `resolve::resolve_sheet` にのみ渡していたが、`t="d"` 対応(Issue #58/PR #80レビュー指摘2)により、シートごとの `parse::parse_worksheet` 呼び出し(フェーズ3)にも同じ値を渡すようになった——[parse/worksheet.md](parse/worksheet.md) 参照
- ルーティングプラン構築後、rels読み込みに使ったリーダーと [`parse::RelationshipMap`](parse/relationships.md) をスコープアウトさせ破棄する（architecture.md「フェーズ1完了時にルーティングマップ構築後、`_rels` の一時バッファを破棄する」の実装）
- ルーティングプラン確定後、シートループに入る前に [`SharedStringTable`](parse/shared_strings.md) と [`StyleSheet`](model/style.md) を一度だけ構築する
- シートごとに [`model::Sheet::new`](model/sheet.md) で空シートを構築し、対応するエントリを [`parse::parse_worksheet`](parse/worksheet.md) に渡してストリームでセルを挿入させ（フェーズ3）、その出力を [`resolve::resolve_sheet`](resolve/mod.md) へ渡して解決する（フェーズ4）
- **フェーズ3.5**(Issue [#65](https://github.com/MinamiyamaKotaro/xlsxparser/issues/65)): フェーズ3が当該シートの `<drawing r:id="...">` を収集していれば、それを `Vec<model::Image>` まで解決する — ワークシート自身の `_rels` を読んで `drawingN.xml` を特定し、[`parse::parse_drawing`](parse/drawing.md) でパースした上で、`drawingN.xml` 自身の `_rels` を読んで各 `<xdr:pic>` の `r:embed`/ハイパーリンク `r:id` をターゲットパスへ解決する。`Internal` なターゲットは[`ZipContainer::has_entry`](container/mod.md)でバイト列を一切読まずZIP内の存在のみ検証する。`r:embed` が解決できない場合はfail-fast(`Error::DanglingRelationship`)だが、画像自体のハイパーリンクが解決できない場合は `Image::hyperlink: None` に縮退させる(PR #66レビュー。この非対称性の理由は `resolve_sheet_images` のdocコメント参照)。全ステップがZIP I/Oを伴うため `resolve/` ではなくここに置く([drawing.md](parse/drawing.md) の依存関係参照)。フェーズ3とフェーズ4の間で実行するが、どちらともデータ依存はない(`resolve_sheet` は `Sheet::images` に一切触れない)ため、実行順序自体は本質的ではない
- **フェーズ3.5、セルのハイパーリンク**(Issue #95): `resolve::resolve_sheet`(結合を確定させるフェーズ4)の後に実行し、フェーズ3が当該シートの `pending_hyperlinks` を1件以上収集していれば、各 `r_id`(存在する場合)をワークシート自身の `_rels` に対して解決し、生のTarget文字列を得る——`_rels` の読み込み自体、`r_id` を実際に持つエントリが1件以上ある場合にのみ行う(`location` のみのシートはこのステップでZIPに一切触れない)。画像の解決と異なり、`Internal` なターゲットのZIPエントリとしての実在確認は意図的に**行わない**し、外部URLも文字列をコピーする以上のことは一切しない——Issue #95で明示的に決めた「ハイパーリンクはdiffのために捕捉するだけで、追従はしない」というスコープそのもの。`_rels` に `r_id` が見つからない場合(壊れたファイル)はシート全体を失敗させず `target: None` に縮退させる——`Image::hyperlink` が既に採っているのと同じトレードオフ。得られた `Vec<model::HyperlinkRange>` は1回の呼び出しで [`resolve::hyperlink::resolve`](resolve/hyperlink.md)(検証 + `Sheet::finalize_hyperlinks`。いずれも純粋関数)へ渡す——画像と異なり、このステップ自身のZIP I/Oはバッチを**構築するだけ**の自己完結したヘルパー(`resolve_sheet_hyperlinks`)に閉じており、`Sheet` への登録自体はここで直接行わず(I/O非依存の)`resolve::hyperlink::resolve` に委ねる。その検証はI/Oの配管ではなく本物のドメインロジックであるため
- 全シートの処理完了後、[`SharedStringTable`](parse/shared_strings.md) と [`StyleSheet`](model/style.md) をスコープアウトさせ破棄し、[`model::Workbook::new`](model/workbook.md) で最終モデルを構築して返す
- **含まない責務**: 各フェーズそのもののロジック（ZIP展開・サニタイズは `container/`、XML構造の解釈は `parse/`、意味解決は `resolve/`）、行単位のXMLノード破棄（フェーズ3の内部詳細であり `parse/worksheet.rs` が担う。architecture.md「`pipeline.rs` はこれを制御しない」）、JSON生成そのもの（[`json.rs`](json.md)。呼び出すかどうかの設計上の位置づけはオープンクエスチョン1参照）

## 主要な型・関数（案）

```rust
use crate::container::sanitize::SizeLimits;
use crate::container::ZipContainer;
use crate::error::Error;
use crate::model::sheet::{Sheet, SheetVisibility};
use crate::model::style::StyleSheet;
use crate::model::workbook::Workbook;
use crate::parse::shared_strings::SharedStringTable;
use crate::{container, model, parse, resolve};
use std::io::{Read, Seek};

// パッケージルート自身の関係パーツ。xl/workbook.xmlとは異なりOPC仕様が
// パスそのものを固定する唯一の例外（Issue #55）。
const PACKAGE_RELS_PATH: &str = "_rels/.rels";
const OFFICE_DOCUMENT_REL_TYPE_SUFFIX: &str = "/relationships/officeDocument";
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
/// シーク可能性を要求する）をそのまま引き継ぐため。`limits` は Zip Bomb対策
/// のサイズ上限（[lib.md](lib.md) `SizeLimits`）で、`lib.rs` の
/// `parse_workbook`/`parse_workbook_reader` からは `SizeLimits::default()`
/// が、`parse_workbook_with_limits`/`parse_workbook_reader_with_limits` から
/// は呼び出し側が指定した値がそのまま渡る。
pub(crate) fn run<R: Read + Seek>(reader: R, limits: SizeLimits) -> Result<Workbook, Error> {
    let mut container = ZipContainer::open_reader(reader)?
        .with_max_entry_size(limits.max_entry_size)
        .with_max_total_size(limits.max_total_size);

    // --- フェーズ1: リレーションシップの解決とルーティングプラン構築 ---
    // まずパッケージルート自身の _rels/.rels を読み、officeDocument関係から
    // workbookパーツの実際のパスを解決する(Issue #55。詳細は本文参照)。
    let package_rels_reader = container
        .get_entry(PACKAGE_RELS_PATH)?
        .ok_or_else(|| Error::MissingRelationshipPart(PACKAGE_RELS_PATH.to_string()))?;
    let package_relationships =
        parse::parse_relationships(package_rels_reader, "", PACKAGE_RELS_PATH)?;
    let workbook_path = package_relationships
        .values()
        .find(|r| r.rel_type.ends_with(OFFICE_DOCUMENT_REL_TYPE_SUFFIX))
        .map(|r| r.target.clone())
        .ok_or_else(|| Error::InvalidPackage(PACKAGE_RELS_PATH.to_string()))?;
    drop(package_relationships);

    let (workbook_rels_path, workbook_dir) = rels_path_for(&workbook_path);
    let rels_reader = container
        .get_entry(&workbook_rels_path)?
        .ok_or_else(|| Error::MissingRelationshipPart(workbook_rels_path.clone()))?;
    let relationships =
        parse::parse_relationships(rels_reader, workbook_dir, &workbook_rels_path)?;

    let workbook_reader = container
        .get_entry(&workbook_path)?
        .ok_or_else(|| Error::InvalidPackage(workbook_path.clone()))?;
    let parsed_workbook = parse::parse_workbook_xml(workbook_reader, &workbook_path)?;
    let date1904 = parsed_workbook.date1904;

    let mut routes = Vec::with_capacity(parsed_workbook.sheets.len());
    for entry in parsed_workbook.sheets {
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
    // セルスタイルを一切使わないワークブックは、OOXML上 styles.xml
    // パーツを持つことを要求されない(Issue #54)——実際にこれを完全に
    // 省略する第三者製ツールが確認されており、Excel自身も他の読み込み
    // ツールも、スタイルなしへフォールバックしてこの種のファイルを
    // 受理する。
    let styles_path = relationships
        .values()
        .find(|r| r.rel_type.ends_with(STYLES_REL_TYPE_SUFFIX))
        .map(|r| r.target.clone());

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
    let stylesheet = match styles_path {
        Some(path) => {
            let reader = container
                .get_entry(&path)?
                .ok_or_else(|| Error::InvalidPackage(path.clone()))?;
            parse::parse_styles(reader, &path)?
        }
        // リレーションシップ自体が存在しない——styles.xmlパーツが本当に
        // 存在しないケース(上記の「リレーションシップは存在するが実体
        // パーツが欠落」とは異なり、こちらは引き続きエラーのまま)。
        // 存在しないStyleIdを参照できるセルは無いため、空のStyleSheet
        // へグレースフルに縮退する(Issue #54)。
        None => StyleSheet::new(),
    };

    // --- シートごとにフェーズ3（ストリームパース）→フェーズ4（解決）---
    let mut sheets = Vec::with_capacity(routes.len());
    for route in routes {
        let mut sheet = Sheet::new(route.name, route.visibility);
        let reader = container.get_entry(&route.worksheet_path)?.ok_or_else(|| {
            Error::DanglingRelationship { r_id: route.worksheet_path.clone() }
        })?;
        let output = parse::parse_worksheet(
            reader,
            &route.worksheet_path,
            &mut sheet,
            date1904,
            limits.max_cells_per_sheet,
        )?;
        resolve::resolve_sheet(
            &mut sheet,
            &output.pending_shared_strings,
            &shared_string_table,
            &output.pending_styles,
            &stylesheet,
            date1904,
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
  - `_rels/.rels`（パッケージルート自身の関係パーツ）不在 → `Error::MissingRelationshipPart`（フェーズ1で最初に読む必須パーツ。Issue #55）
  - `_rels/.rels` は存在するが `officeDocument` タイプの関係が1件も無い → `Error::InvalidPackage`（workbookパーツのパスをそもそも解決できない。Issue #55）
  - workbookパーツの `_rels`（`officeDocument` 関係のターゲットから導出したパス）不在 → `Error::MissingRelationshipPart`
  - workbookパーツ本体（同上のパス）不在、または rels が指す `styles.xml`/`sharedStrings.xml` の実体パーツが不在 → `Error::InvalidPackage`(リレーションシップ自体は約束していたパーツが実際には無い——破損・切り詰められたパッケージ)
  - `workbook.xml` の `<sheet r:id="...">` が指す r:id が `RelationshipMap` に存在しない、または rels が指す worksheet の実体パーツが不在 → `Error::DanglingRelationship`
- `sharedStrings.xml` *または* `styles.xml` に対応するリレーションシップ自体が全く見つからない場合(リレーションシップは存在するが対象の実体パーツが欠落している上記のケースとは異なる)はエラーにしない: `sharedStrings.xml` は `SharedStringTable::default()`(空テーブル)、`styles.xml` は `StyleSheet::new()`(空テーブル)へそれぞれフォールバックする——文字列セルを持たないブック/セルスタイルを一切使わないブックは、それぞれのパーツを持つ必要が元々ない。`styles.xml` はIssue #54でこのグレースフルデグラデーション方針に合流した——セルスタイルなしのブックに対してこのパーツを完全に省略する第三者製ツールが実際に確認されており(calamine自身のテストコーパスで検証済み)、有効なはずのパッケージを過度に厳格に拒否していたと判断した

## テスト方針

- 正当な最小構成の `.xlsx` 相当ZIP（1シート、数値・共有文字列参照・結合セルを含む）を `run` に渡し、`Ok` で期待する `Workbook` が得られることの確認（統合テスト）
- `_rels/.rels` が存在しないZIPに対し `Error::MissingRelationshipPart` を返すことの確認（Issue #55）
- `_rels/.rels` は存在するが `officeDocument` タイプの関係が無いZIPに対し `Error::InvalidPackage` を返すことの確認（Issue #55）
- workbookパーツが `xl/workbook.xml` ではなくパッケージルート直下（`workbook.xml`）にあり、`_rels/.rels` 経由でのみ発見可能なZIPでも `run` が正しく解決できることの確認(実例として `tests/fixtures/other/minimal_package.xlsx` ——calamineテストコーパス由来。同ディレクトリは`.gitignore`対象でリポジトリには含まれないため、統合テスト自体は同じ構造を再現した手書きフィクスチャ(`tests/fixtures/builder.rs` 経由)を用いる。Issue #55)
- `xl/_rels/workbook.xml.rels` が存在しないZIPに対し `Error::MissingRelationshipPart` を返すことの確認
- `xl/workbook.xml` が存在しないZIPに対し `Error::InvalidPackage` を返すことの確認
- `workbook.xml` の `<sheet r:id="...">` が指す r:id が rels 内に存在しない場合に `Error::DanglingRelationship` を返すことの確認
- rels 内の styles 関係が指す実体ファイルがZIP内に存在しない場合に `Error::InvalidPackage` を返すことの確認
- rels 内に worksheet 関係の実体ファイルが存在しない場合に `Error::DanglingRelationship` を返すことの確認
- `sharedStrings.xml` パーツ自体が存在しない（文字列セルを一切含まない）ブックでもエラーにならず、空の `SharedStringTable` で正常に完走することの確認
- **`.../relationships/styles` タイプのリレーションシップ自体が全く存在しない(実体パーツが欠落しているのとは異なる)ZIPでも、空の `StyleSheet` へフォールバックして正常に完走することの確認**(Issue #54。直上の `Error::InvalidPackage` ケースとの対比)
- 複数シートを持つブックで、各シートが `xl/workbook.xml` の `<sheets>` 定義順で `Workbook.sheets()` に格納されることの確認（[model/workbook.md](model/workbook.md) のソース順維持方針との結線）
- 途中のシート（例: 2枚目）のパースが失敗する場合に、1枚目が正常に処理済みであっても `Workbook` 全体が返らず `Err` になることの確認（fail closed の回帰テスト）
- 可視性が `Hidden`/`VeryHidden` のシートを含むブックでも、全シートが除外されずに `Workbook` へ含まれることの確認（[model/workbook.md オープンクエスチョン1](model/workbook.md) との結線）
- `run` に `DEFAULT_MAX_UNCOMPRESSED_SIZE` より小さい `max_entry_size` を持つ `SizeLimits` を渡した場合、通常なら成功するはずの入力が `Error::ZipBombDetected` になることの確認（`SizeLimits` が実際に `ZipContainer` まで橋渡しされていることの結線テスト。`with_max_entry_size`/`with_max_total_size` 自体のロジック検証は [container/mod.md](container/mod.md) 側の責務）
- `run` に小さい `max_cells_per_sheet` を持つ `SizeLimits` を渡した場合、`Error::TooManyCells` になることの確認（`SizeLimits.max_cells_per_sheet` が実際に `parse::parse_worksheet` まで橋渡しされていることの結線テスト。実際のカウント・打ち切りロジック自体は [parse/worksheet.md](parse/worksheet.md) 側の責務。Issue #88）

## 未決事項 / オープンクエスチョン

1. **`json.rs` を `run` 内から呼ぶかどうか**: [architecture.md](architecture.md) の `pipeline.rs` 節は「`resolve` で解決した結果を `json.rs` でシリアライズする」と5フェーズ全体の流れを説明する一方、[model/workbook.md](model/workbook.md) は既に「`lib.rs` の公開API（`parse_workbook(path) -> Result<Workbook>`）の返り値そのものになる」と明記しており、`Workbook`（JSONではなく構造化データ）が主要な返り値であることを前提としている。本設計はこの矛盾を、architecture.md の記述を「クレート全体が提供する5フェーズの機能」を示す概念的な説明と解釈し、`run` 自体はフェーズ1〜4（`Workbook` を返す）までを担い、フェーズ5（JSON化）は [`json.rs`](json.md) が提供する別関数として `Workbook` から明示的に呼び出す2段構成と解決した。`lib.rs`（未設計）がこの2段構成をどう公開するか（`parse_workbook` と `parse_workbook_json` を別々に公開する、`Workbook` に `to_json` メソッドを生やす等）は `lib.rs` の設計時に確定させる。
2. **`lib.rs` との結線**: `run` は `pub(crate)` とし、パス文字列から `std::fs::File` を開いて渡す薄いラッパーを `lib.rs` 側の公開関数として想定しているが、`Read + Seek` を実装する任意の入力（インメモリバッファ等）をどこまで `lib.rs` の公開APIとして許容するかは `lib.rs` の設計時に確定させる。
3. ~~`xl/workbook.xml` / `xl/_rels/workbook.xml.rels` の固定パス依存~~ → **部分的に解決**（Issue [#55](https://github.com/MinamiyamaKotaro/xlsxparser/issues/55)）: workbookパーツのパスは `_rels/.rels` の `officeDocument` 関係から解決するよう改めた（本文参照）。ただしこの解決は `Relationship.rel_type`（関係タイプ文字列の前方一致）のみに基づくものであり、`[Content_Types].xml` のContent-Type宣言は依然として一切参照していない。厳密なOPC準拠のためには `[Content_Types].xml` を介した二重検証（パーツのContent-Typeが期待どおりか）を行うべきという意見もありうるが、関係タイプによる解決だけで実用上は十分機能しており（`minimal_package.xlsx` を含め、これまで確認できた全ケースを解決できている）、優先度は低いままとした。
4. **個々のシートのパース失敗時の耐性**: 現状は1シートでもエラーがあれば `run` 全体を `Err` で返す設計（fail closed）を採用している。要求仕様書に「壊れたシートをスキップして他シートだけ返す」という要件はないためこの設計としたが、将来的にエラー耐性モード（部分的に読めたデータを返す）の要求が生じた場合は見直しが必要になる。
5. **並行処理**: 現状シートを1つずつ逐次処理する設計（依存関係セクションで述べたとおり `container::get_entry` の逐次アクセス制約とも自然に合致する）。要求仕様書に並列化要件はないが、複数シートを持つ大規模ブックに対するパフォーマンス最適化として、各シートのバイト列を先にすべてメモリへ読み出したうえでスレッドプール等により並列処理する余地はあるか（この場合ストリーミング方針とはトレードオフになる）は、実装後のプロファイリング結果を踏まえて再検討する。
6. ~~`BufRead` 要求と `container::get_entry` の返り値の型の不整合~~ → **実装時に解決**: `parse::parse_*` の各関数はいずれも `impl BufRead` を要求する（quick-xmlの `Reader::read_event_into` がこれを必要とするため）が、`container::get_entry` が返す `BoundedReader<'_, impl Read + '_>` は `Read` のみを実装する。`run` は `get_entry` から得た各readerを `parse::parse_*` へ渡す前に `std::io::BufReader::new(..)` でラップする。上記コードブロックのドラフトはこの点を反映していないが、これは未決の設計論点だったからではなく、`container/` と `parse/` がそれぞれ独立に設計されており、両者を実際に結合する `pipeline.rs` のコンパイルまでこの境界面が検証されていなかったための単純な抜けである。
7. **`theme{N}.xml`（[Issue #76](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76)）の読み込み統合**: [`parse/theme.md`](parse/theme.md) は `theme{N}.xml` パーツの実体パス解決・読み込みタイミングを本ファイルの責務として持ち越している。想定される形は `styles_path` と同じくフェーズ1で `THEME_REL_TYPE_SUFFIX = "/relationships/theme"` によりリレーションシップから解決し、`styles_path` 同様パーツ自体が存在しない場合は `Workbook::theme = None` へグレースフルに縮退させること（本文の `styles_path`/`shared_strings_path` 解決パターンをそのまま踏襲）。ただし [Issue #76 設計提案](https://github.com/MinamiyamaKotaro/xlsxparser/issues/76#issuecomment-5352309575)が要求する「pay-for-what-you-use」——`stylesheet` が `ColorRef::Theme` を1件も含まない場合は `theme{N}.xml` のI/O・パースそのものを完全にスキップする——を実現するには、`stylesheet`（フェーズ1〜3の間で既に構築済み）を走査して `ColorRef::Theme` の有無を判定するか、[`parse/styles.rs`](parse/styles.md) が `StyleSheet` 構築と同時に `uses_theme_color: bool` のようなフラグを一緒に返す形にするかの選択が残る。後者は `parse/styles.rs` の返り値の形を変える必要があり、[styles.md](parse/styles.md) 側の設計変更を伴うため、実装時に確定させる。
