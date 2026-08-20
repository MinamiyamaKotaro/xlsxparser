# `src/error.rs` 設計書

*[English](error.en.md)*

`src/error.rs` に対応する設計書。[architecture.md](architecture.md) が定義する5フェーズ・パイプライン全体で共有する、ライブラリ共通のエラー型を定義する。[model/cell.md](model/cell.md)（`CellRef::from_a1`）、[model/sheet.md](model/sheet.md)（不正な結合範囲の検証）、[model/workbook.md](model/workbook.md)（`parse_workbook` の `Result::Err`）が本ファイルの型への依存を前提として書かれている。

## 責務・スコープ

- クレート全体で使用する単一のエラー列挙型 `Error`（および `pub type Result<T> = std::result::Result<T, Error>;`）を定義する
- 各フェーズ（rels解決・サニタイズ・ストリームパース・分析/遅延解決）で発生しうる失敗系を、呼び出し元がハンドリングに必要な情報（対象パス・不正値など）とともに表現する
- `std::error::Error` を実装し、外部クレートのエラー（`quick-xml` など）を型消去した `#[source]` として保持することで、根本原因をチェーンで追跡可能にしつつ、そのクレート自体をパブリック依存にしない
- **含まない責務**: エラーからの回復処理・リトライ（呼び出し側の責務）、エラーメッセージの多言語化（本ライブラリはRustのエラー文字列を1種類のみ提供し、i18nは呼び出し側でエラー種別に応じて行う想定）

## 主要な型（案）

```rust
use std::path::PathBuf;

/// クレート全体の共通 Result エイリアス。
pub type Result<T> = std::result::Result<T, Error>;

/// ライブラリ全体で使用する共通エラー型。`parse_workbook` の `Result::Err` を
/// はじめ、全モジュールの失敗系はこの型に集約する。将来の変種追加が破壊的変更に
/// ならないよう `#[non_exhaustive]` を付与する（オープンクエスチョン2参照）。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    // --- フェーズ1: リレーションシップの解決 ---
    /// `xl/_rels/workbook.xml.rels` 等、必須の rels パーツがZIP内に存在しない。
    #[error("required relationship part not found: {0}")]
    MissingRelationshipPart(String),

    /// `workbook.xml` の `<sheet r:id="...">` が参照する r:id が rels 内に
    /// 存在しない、または rels が指す実体ファイルがZIP内に存在しない。
    #[error("dangling relationship reference: r:id={r_id}")]
    DanglingRelationship { r_id: String },

    // --- フェーズ2: サニタイズ ---
    /// 展開後の総サイズが上限を超えた（Zip Bomb対策、要求仕様書2章）。
    #[error("zip bomb detected: uncompressed size {actual} bytes exceeds limit {limit} bytes")]
    ZipBombDetected { limit: u64, actual: u64 },

    /// ZIPエントリ名が展開先ディレクトリ外へのパストラバーサルを含む（Zip Slip対策）。
    #[error("path traversal detected in zip entry: {entry_name}")]
    ZipSlipDetected { entry_name: String },

    /// ZIPアーカイブとして破損している、または `[Content_Types].xml` /
    /// `xl/workbook.xml` など .xlsx (OPC) パッケージとして必須のパーツを欠く。
    #[error("not a valid .xlsx package: {0}")]
    InvalidPackage(String),

    // --- フェーズ3: ストリームパース ---
    /// XMLとして構文的に不正な内容を検知した（XMLパーサーのエラーを包む）。
    /// `source` は具体的なパーサーのエラー型（例: `quick_xml::Error`）を直接
    /// 持たず `Box<dyn Error>` で型消去する（理由は本コードブロック直後の解説を参照）。
    #[error("XML parse error in {path}: {source}")]
    XmlParse {
        path: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    /// 必須の要素・属性がXML上に存在しない（例: `<c>` の `r` 属性欠落）。
    #[error("missing required element/attribute `{name}` in {path}")]
    MissingRequiredElement { path: String, name: &'static str },

    /// `<!DOCTYPE ...>` 宣言を検知し無条件に拒否した（XXE対策。
    /// [parse/mod.md](parse/mod.md) `read_event` が生成する）。OOXMLの
    /// `_rels`/`workbook.xml`/`sharedStrings.xml`/`styles.xml`/`sheetX.xml`
    /// はいずれも仕様上DOCTYPE宣言を持たないため、正当な `.xlsx` に対して
    /// 本バリアントが発生することはない（[セキュリティレビュー](../security/design-review.md)
    /// Finding 1を反映して新設）。
    #[error("DOCTYPE declaration rejected in {path} (XXE defense)")]
    DoctypeRejected { path: String },

    /// 1シートに実際に挿入されたセル数(`Sheet::insert_cell` に到達した
    /// セルのみ。値・スタイル・共有文字列参照のいずれも持たない `<c>` は
    /// `flush_cell` が無料で捨てるためカウントされない)が
    /// `SizeLimits::max_cells_per_sheet` を超えた(Issue
    /// [#88](https://github.com/MinamiyamaKotaro/xlsxparser/issues/88))。
    /// `TooManyMergedRanges`/`TooManyColumnWidthRanges` (バッチ収集後に
    /// 一括チェック)とは異なり、`parse/worksheet.rs` が `<c>` を
    /// ストリーミングする最中に逐次チェックする——セルの場合、メモリコスト
    /// は挿入された瞬間に発生するため、収集し終えてからのチェックでは
    /// 手遅れになる。上限値は[container/sanitize.md](container/sanitize.md)
    /// の `SizeLimits::max_cells_per_sheet` 参照。
    #[error("too many cells in {path}: {count} exceeds limit {limit}")]
    TooManyCells {
        path: String,
        count: usize,
        limit: usize,
    },

    // --- フェーズ4: 分析と遅延解決 ---
    /// A1形式のセル参照文字列が不正（構文エラー・桁溢れ・空文字列など、
    /// model/cell.md の `CellRef::from_a1` が返す）。
    #[error("invalid cell reference: {0:?}")]
    InvalidCellRef(String),

    /// 共有文字列テーブルの範囲外インデックス（`t="s"` が指す値がSSTの長さを超える）を参照した。
    #[error("shared string index {index} out of bounds (table len={len})")]
    SharedStringIndexOutOfBounds { index: usize, len: usize },

    /// 存在しないスタイルID（`cellXfs` のインデックス）を参照した。
    #[error("invalid style id: {0}")]
    InvalidStyleId(u32),

    /// 結合セル範囲が不正（他の結合範囲との重複、開始・終了座標の大小関係が
    /// 逆転しているなど。model/sheet.md `insert_merge` 呼び出し前の検証で使用）。
    #[error("invalid merged cell range {start}:{end}: {reason}")]
    InvalidMergedRange {
        start: String,
        end: String,
        reason: String,
    },

    /// 1シート内の`<mergeCell>`件数が `resolve::merge::MAX_MERGE_REGIONS`
    /// を超えた（実装時に追加。[セキュリティコードレビュー Finding 1](security/code-review.md)
    /// を受けて。[resolve/merge.md オープンクエスチョン2の追記](resolve/merge.md)参照）。
    #[error("too many merged cell ranges in one sheet: {count} exceeds limit {limit}")]
    TooManyMergedRanges { count: usize, limit: usize },

    /// `<col>` の列幅範囲が不正（他の範囲と重複している）。`InvalidMergedRange`
    /// と同じ形（Issue #39。[resolve/column_width.md](resolve/column_width.md)参照）。
    #[error("invalid column width range {min}:{max}: {reason}")]
    InvalidColumnWidthRange { min: u32, max: u32, reason: String },

    /// 1シート内の`<col>`範囲件数が `resolve::column_width::MAX_COLUMN_WIDTH_RANGES`
    /// を超えた。`TooManyMergedRanges` と同じ理由(Zip Bombのバイト数上限だけでは
    /// 範囲件数を抑えられない)によるもので、O(N²)のリスクを防ぐためではない
    /// ([resolve/column_width.md](resolve/column_width.md)参照)。
    #[error("too many column width ranges in one sheet: {count} exceeds limit {limit}")]
    TooManyColumnWidthRanges { count: usize, limit: usize },

    // --- フェーズ5: JSON生成 ---
    /// JSONへのシリアライズに失敗した（`serde_json` が返すエラーを包む）。
    /// `source` は `XmlParse::source` と同じ理由で `Box<dyn Error>` として
    /// 型消去する（[json.md](json.md) の設計に伴い新設。PR #10 レビューを
    /// 反映）。実際には `json.rs` が非有限浮動小数点数を事前にフォール
    /// バックさせてから `serde_json` へ渡すため、値の内容に起因する失敗は
    /// 想定していない。主に `Write` 実装側のI/Oエラーの伝播経路として使う。
    #[error("JSON serialization error: {source}")]
    JsonSerialize {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    // --- 全フェーズ共通 ---
    /// ファイルI/O由来のエラー（対象ファイルが開けない、読み込めないなど）。
    /// `path` はファイルパスを経由しない入力（例: `Cursor<Vec<u8>>` などの
    /// インメモリバッファ、または将来 `lib.rs` が `Read` トレイト入力を
    /// 受け付ける場合）ではパスが存在しないため `Option` とする。`Some` の
    /// 場合は `io_path_suffix` ヘルパー経由でDisplay文言に付加し、エラー
    /// テキストだけで原因ファイルを特定できるようにする（実装時に確定。
    /// PR #19 レビュー指摘を反映）。
    #[error("I/O error{}: {source}", io_path_suffix(path))]
    Io {
        path: Option<PathBuf>,
        #[source]
        source: std::io::Error,
    },
}

/// `Error::Io` の `path`（`Option`）をDisplayメッセージの接尾辞として整形する
/// （`None` の場合は空文字列）。
fn io_path_suffix(path: &Option<PathBuf>) -> String {
    match path {
        Some(p) => format!(" (path: {})", p.display()),
        None => String::new(),
    }
}

/// パス情報を持たないI/Oエラー（インメモリバッファ由来など）を `?` で
/// 変換するための実装。パスが判明している場合は `Error::Io { path: Some(..),
/// source }` を明示的に構築し、パス情報を残すこと（実装時に追加。
/// PR #19 レビュー指摘を反映）。
impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Error::Io { path: None, source }
    }
}
```

`InvalidPackage` は、ZIP展開そのものの失敗（破損アーカイブ等）を含む暫定的な受け皿としている。`container/` の設計（使用するZIP操作クレートの選定）が確定した際に、当該クレートのエラー型を `#[source]` として保持する専用バリアントへ分離するかを見直す（オープンクエスチョン1参照）。

`XmlParse::source` を具体的なパーサーの型（例: `quick_xml::Error`）ではなく `Box<dyn std::error::Error + Send + Sync + 'static>` として保持しているのは、`Error` が `#[non_exhaustive]` であっても各バリアントの名前付きフィールドの型自体は外部から参照可能であり、フィールドに具体的な外部クレート型を置くとそのクレートが事実上パブリック依存になるためである。パブリック依存になると、当該クレートのメジャーバージョンアップが本ライブラリ側の破壊的変更を誘発し、利用者側も `Error::XmlParse { source, .. }` を扱うために当該クレートを直接依存に追加せざるを得なくなる。型消去することで、将来 `parse/` が採用するXMLパーサーを変更・更新してもパブリックAPIに影響しない（PR #6 レビュー指摘を反映）。同様の理由から `Io::source`（`std::io::Error`）は標準ライブラリの型でありパブリック依存の問題が生じないため、型消去せずそのまま保持する。`JsonSerialize::source` も同じ理由で `serde_json::Error` を直接保持せず型消去する（[json.md](json.md) の設計時に追加。PR #10 レビューを踏まえた設計変更に伴う）。

## 依存関係

- 依存先: なし（`model/` を含むクレート内の他モジュールに依存しない、最も基底のリーフモジュール。`error.rs` が他モジュールを参照すると循環依存になるため）。外部クレートとしては `thiserror`（エラー型定義の定型コード削減）にのみ依存する。`quick-xml` には依存しない。`XmlParse::source` はXMLパーサーの具体的なエラー型を直接保持せず `Box<dyn std::error::Error + Send + Sync + 'static>` として型消去するため、`quick-xml`（や将来 `parse/` が採用しうる他のXMLパーサー）はパブリック依存にならない（詳細は主要な型セクションの解説を参照。PR #6 レビュー指摘を反映）。
- 依存元: クレート内のほぼ全モジュール（`container/`, `parse/`, `model/`, `resolve/`, `pipeline.rs`, `lib.rs`）。[`json.rs`](json.md) は `Error::JsonSerialize`（`serde_json`/I/O由来の失敗のみを表現。PR #10 レビューを踏まえて追加）を除き本型を新規に生成しない。

`thiserror` はコンパイル時のみのproc-macro依存であり、ランタイムの実行バイナリサイズや速度への影響がないため、要求仕様書1章が掲げる「軽量かつ高速」という方針と矛盾しない。

## エラー処理方針

- `error.rs` 自身がエラーを生成する処理は持たない（型定義のみ）。以下は本型を使う側全体に適用される方針。
- ライブラリ内部では `panic!` / `unwrap()` / `expect()` を使用しない。パース対象は常に信頼できない外部ファイルであるため、あらゆる想定外入力は `Error` のいずれかのバリアントとして呼び出し側に伝播させる（[model/cell.md](model/cell.md) のエラー処理方針と同一の原則）。
- 外部クレート由来のエラー（`quick-xml` 等）は握りつぶさず、`#[source]` として保持し `std::error::Error::source()` 経由で根本原因を追跡可能にする。ただしパブリック依存を避けるため、具体的な外部クレート型をフィールドに直接置くのではなく `Box<dyn std::error::Error + Send + Sync + 'static>` で型消去して保持する（`XmlParse` 参照。標準ライブラリの型である `std::io::Error` はこの限りではない）。
- どのファイル・どの座標で発生したエラーかを呼び出し側がログ出力やデバッグに使えるよう、可能な限りバリアントにコンテキスト情報（`path`, `r_id`, `index` など）を持たせる。

## テスト方針

- 各バリアントの `Display`（`#[error(...)]` メッセージ）が意図した文字列を生成することの確認。`Io` は `path: Some(..)` / `path: None` の両方（`io_path_suffix` の分岐）を確認する
- `std::error::Error::source()` が `XmlParse` / `Io` / `JsonSerialize` について正しく根本原因を返すことの確認
- `From<std::io::Error>` が `Error::Io { path: None, .. }` を生成し、`?` 経由で正しく伝播することの確認
- `#[non_exhaustive]` により、クレート利用者側の `match` で `_ =>` アームなしにコンパイルできない（＝将来のバリアント追加が破壊的変更にならない）ことをドキュメント上明記し、コンパイル可否のテストは行わない（`#[non_exhaustive]` 自体はコンパイラが保証する言語機能のため）
- 本ファイル単体のロジックテストは型定義のみのため最小限とし、実際のバリアント生成・伝播の検証は各生成元モジュール（`model/cell.rs` の `from_a1` 等）のテストで行う

## 未決事項 / オープンクエスチョン

1. ~~ZIP操作に使用する外部クレートの選定~~ → **解決**: `zip` クレート（v8。[container/mod.md](container/mod.md) 参照）。`InvalidPackage(String)` は専用の `#[source]` 保持バリアントにはせず、現状の `String` 受け皿のままとした（`container/mod.rs` は `zip::result::ZipError` を単純に文字列化しているのみ）。呼び出し側にとって粒度が粗すぎる場合は見直す。
2. **`#[non_exhaustive]` の是非**: 将来的なバリアント追加を破壊的変更にしないための一般的なプラクティスとして仮採用しているが、本クレートが1.0未満のうちはバージョニング上バリアント追加自体が破壊的変更にならない（Cargoのセマンティックバージョニング規約）ため、正式リリース方針が固まるまでは不要という判断もありうる。
3. **エラーの粒度**: 現在は「どのフェーズで何が起きたか」を1階層のフラットな enum で表現しているが、バリアント数が今後さらに増えた場合に `Error::Xml(XmlError)` のようなフェーズ単位のネストしたサブenumへ分割するかは未決定。ネスト化すると呼び出し側のマッチングが `Error::Xml(XmlError::MissingRequiredElement(...))` のように深くなり書きにくくなるというデメリットもあるため、ライブラリの規模が極端に大きくならない限りは現状のフラットなenumを維持するメリットの方が大きい（PR #6 レビュー指摘を反映）。
4. ~~`InvalidCellRef` / `InvalidMergedRange` の入力値保持方法~~ → **解決**: [model/cell.md](model/cell.md) の `CellRef` 型そのものは保持せず、現状設計通り `String`（元の入力文字列やA1表記への変換結果）を保持する方針とする。`error.rs` を他モジュール（`model/` を含む）に依存しない最も基底のリーフモジュールとして維持するという依存関係セクションの原則を優先するため。仮に `CellRef` をフィールドに含めると `error.rs → model::cell` の依存が生じ、一方で `CellRef::from_a1` は `crate::error::Error` に依存しているため、モジュール間で循環依存が発生してしまう（PR #6 レビュー指摘を反映）。
5. **`std::error::Error` 実装のためのMSRV**: `thiserror` のバージョン選定（`std::error::Error::source()` の扱いなど）は、クレート全体のMSRV（Minimum Supported Rust Version）方針が未確定のため、`Cargo.toml` 整備時にあわせて確定させる。
