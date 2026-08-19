# `container/mod.rs` 設計書

*[English](mod.en.md)*

`src/container/mod.rs` に対応する設計書。[architecture.md](../architecture.md) が定義する `container/` の責務「ZIP(OPC)展開のエントリポイント、安全なファイル取得」を実装する。`pipeline.rs` はここで定義する `ZipContainer` を所有し、フェーズ間のリソース破棄タイミングを制御する（architecture.md 設計方針3）。

## 責務・スコープ

- ZIP(OPC)アーカイブを開き、中央ディレクトリから全エントリ名を読み取る
- オープン時点で全エントリ名を [`container/sanitize.rs`](sanitize.md) の `validate_entry_path` で一括検証し、不正なエントリ名を含むアーカイブを即座に拒否する（fail closed）
- 個々のエントリの展開済みストリームを、Zip Bomb対策の `BoundedReader`（[sanitize.md](sanitize.md)）で包んだ状態でのみ払い出す「安全なファイル取得」窓口 `get_entry` を提供する
- `HashSet<String>` を裏付けとした、存在確認専用の `has_entry` を提供する。中身ではなく存在の有無だけが必要な呼び出し元にとって、`get_entry` のローカルファイルヘッダ読み込みや `BoundedReader` 構築を回避できる(Issue #65のPRレビュー。`pipeline.rs` の画像アンカー解決が最初の利用者)。このセットは `open_reader` 時点で即座に構築するのではなく、最初の `has_entry` 呼び出し時に遅延構築する — 即時構築だとエントリ数の多いアーカイブで `open_reader` に無視できない(約3〜5%)速度低下が生じ、画像アンカー解決でしか使わない機能のコストをほぼ全ての呼び出し元が負担することになると計測でわかったため
- **含まない責務**: Zip Bomb/Zip Slipの検知ロジックそのもの（`container/sanitize.rs`）、XMLの構文解釈・XXE対策（`parse/`）、`_rels` の内容解釈やシートIDとファイルパスの紐付け（`parse/relationships.rs`）、`[Content_Types].xml` / `xl/workbook.xml` など特定パーツの必須性判断（呼び出し元。本ファイルは「名前を指定されたエントリを安全に取得できるか」のみを扱い、どのパーツが必須かは知らない）

## 主要な型（案）

```rust
use crate::container::sanitize::{
    self, BoundedReader, DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE, DEFAULT_MAX_UNCOMPRESSED_SIZE,
};
use crate::error::Error;
use std::io::{Read, Seek};
use std::path::Path;

/// .xlsx (OPC) パッケージのZIP展開エントリポイント。オープン時に中央
/// ディレクトリの全エントリ名を `sanitize::validate_entry_path` で検証
/// 済みにしておくことで、以降 `get_entry` を通過するエントリ名は常に
/// 安全であることを型として保証する。
///
/// 内部でどのZIP操作クレートの型を保持するかは未選定（オープンクエスチョン1参照）。
pub struct ZipContainer<R> {
    archive: R, // 実際にはZIP操作クレートが提供するアーカイブ型を保持する想定のプレースホルダー
    max_entry_size: u64,
    /// アーカイブ全体を通じた累積展開後サイズの上限（Zip Bomb対策、
    /// [sanitize.md](sanitize.md) 参照。PR #7 レビュー指摘を反映）。
    max_total_size: u64,
    /// これまでに `get_entry` 経由で展開されたバイト数の累計。
    /// `get_entry` が `BoundedReader` へ `&mut` で貸し出す
    /// （依存関係セクション参照）。
    total_read: u64,
}

impl ZipContainer<std::fs::File> {
    /// ファイルパスからアーカイブを開く。
    pub fn open(path: &Path) -> Result<Self, Error> {
        Self::open_reader(std::fs::File::open(path).map_err(|source| Error::Io {
            path: Some(path.to_path_buf()),
            source,
        })?)
    }
}

impl<R: Read + Seek> ZipContainer<R> {
    /// 任意の `Read + Seek` からアーカイブを開く（インメモリバッファ等）。
    /// ZIP形式は末尾の中央ディレクトリを参照するため、シーク可能な入力を要求する
    /// （純粋なストリーミング入力 `Read` のみからは開けない）。
    ///
    /// 中央ディレクトリの読み取りに成功した時点で全エントリ名を
    /// `sanitize::validate_entry_path` で検証する。1件でも不正な場合は
    /// `Error::ZipSlipDetected` を返しアーカイブ全体を拒否する。
    pub fn open_reader(reader: R) -> Result<Self, Error> {
        let _ = reader;
        unimplemented!()
    }

    /// 指定したエントリ名の展開済みストリームを取得する。
    ///
    /// - `name` は呼び出しのたびに `sanitize::validate_entry_path` で再検証する
    ///   （オープン時の検証はアーカイブ自身が持つエントリ名が対象であり、
    ///   `name` 自体は `parse/relationships.rs` 等が rels の相対パスとエント
    ///   リ名を組み合わせて計算した値でありうるため、独立した信頼できない
    ///   入力として扱う。詳細は依存関係セクション参照）。
    /// - アーカイブ内に該当エントリが存在しない場合は `Ok(None)` を返す。
    ///   「存在しない」こと自体は異常系ではなく、それが必須パーツの欠落
    ///   （`Error::InvalidPackage`）なのか、rels参照切れ
    ///   （`Error::DanglingRelationship`）なのかは呼び出し側の文脈でしか
    ///   判断できないため、本メソッドはエラーを構築しない。
    /// - 返すストリームは `BoundedReader` で包まれており、Zip Bomb対策の
    ///   エントリ単体の上限（`max_entry_size`）と、アーカイブ全体の累積
    ///   上限（`max_total_size`）の両方が適用済みである。
    pub fn get_entry(&mut self, name: &str) -> Result<Option<BoundedReader<'_, impl Read + '_>>, Error> {
        sanitize::validate_entry_path(name)?;
        // `archive` フィールドから得るエントリの読み取りストリームと、
        // `total_read` フィールドへの可変参照（`BoundedReader::new` の
        // `cumulative_read` 引数として渡す）は、同一の `self` から
        // 分割借用（disjoint field borrow）することで両立できる。
        // `Cell` 等の内部可変性は不要（sanitize.md 参照）。
        let Self { archive, total_read, max_entry_size, max_total_size, .. } = self;
        let _ = (archive, name, *max_entry_size, total_read, *max_total_size);
        unimplemented!()
    }

    /// アーカイブ内の全エントリ名を列挙する（オープン時に検証済みのものを返す）。
    pub fn entry_names(&self) -> impl Iterator<Item = &str> {
        std::iter::empty()
    }
}

impl<R> ZipContainer<R> {
    /// Zip Bomb対策のエントリ単体サイズ上限を明示的に設定して開く。未指定時は
    /// `DEFAULT_MAX_UNCOMPRESSED_SIZE`（[sanitize.md](sanitize.md)）を使う
    /// 想定（具体的なビルダーAPIの形は未確定。オープンクエスチョン3参照）。
    fn with_max_entry_size(mut self, limit: u64) -> Self {
        self.max_entry_size = limit;
        self
    }

    /// Zip Bomb対策のアーカイブ全体累積サイズ上限を明示的に設定して開く。
    /// 未指定時は `DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE`（[sanitize.md](sanitize.md)）
    /// を使う想定（PR #7 レビュー指摘を反映してオープンクエスチョン4を解決。
    /// 具体的なビルダーAPIの形は `with_max_entry_size` 同様オープンクエスチョン3参照）。
    fn with_max_total_size(mut self, limit: u64) -> Self {
        self.max_total_size = limit;
        self
    }
}
```

## 依存関係

- 依存先: [`container/sanitize.rs`](sanitize.md)（`validate_entry_path`, `BoundedReader`, `DEFAULT_MAX_UNCOMPRESSED_SIZE`, `DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE`）、[`error.rs`](../error.md)。`model/` および `parse/` には依存しない。
- 依存元: `pipeline.rs` のみ。architecture.md 設計方針3（「`container` と `parse` は密に往復するが、この呼び出し順序とリソースのライフサイクル管理は `pipeline.rs` に一元化し、他のモジュールが互いを直接知らなくてよいようにする」）に基づき、`parse/` 配下の各モジュールは `container::ZipContainer` を直接知らず、`pipeline.rs` が `get_entry` で取得したバイト列（ストリーム）を受け渡す。

`get_entry` が `name` を毎回再検証する理由: オープン時の `validate_entry_path` 適用対象はアーカイブが実際に持つエントリ名（中央ディレクトリ由来、静的な文字列）だが、`get_entry` に渡される `name` は `pipeline.rs` 経由で `parse/relationships.rs`（フェーズ1）が `.rels` 内の相対パス表記（例: `../media/image1.png`）とエントリ名を組み合わせて動的に計算した文字列でもありうる。この計算過程に正規化漏れ（`..` の未解決など）があった場合、オープン時検証をすり抜けたパスが `get_entry` に到達しうるため、独立した信頼できない入力として毎回検証する（多層防御）。

`get_entry` が `&mut self` を要求し、かつ返り値の生存期間が `self` の借用に束縛される設計（`impl Read + '_`）は、architecture.md が既に述べる「`container` と `parse` はバイト列取得→パース→次のエントリ取得、という形で密に往復する」という逐次アクセスパターンと自然に一致する。同時に複数エントリを開いたまま処理する必要はない前提を型で表現している。

## エラー処理方針

- `open` / `open_reader` は、ZIPアーカイブとして破損している場合に `Error::InvalidPackage` を返す。使用するZIP操作クレートのエラー型をそのまま `String` 化するか、`error.md` の `XmlParse` と同様に `Box<dyn Error>` として型消去した専用フィールドを持たせるかは、クレート選定後に見直す（オープンクエスチョン1、[error.md オープンクエスチョン1](../error.md) と連動）。
- `open_reader` は、中央ディレクトリ内のいずれかのエントリ名が `validate_entry_path` に失敗した場合、`Error::ZipSlipDetected` を返しアーカイブ全体を拒否する（部分的に安全なエントリのみを使う、という妥協はしない）。
- `get_entry` はエントリ不在を `Result` ではなく `Ok(None)` として表現する（`model::Sheet::get` が空白セルを `None` で表すのと同じ設計原則。[model/sheet.md](../model/sheet.md) 参照）。`has_entry` も同様に `Ok` の中身を素の `bool` として表現する — `Result` で包んでいるのは `validate_entry_path` 自体の失敗(動的に計算された信頼できない `name` がZip Slip検証に落ちるケース)のためであり、「該当エントリが無い」とは別の関心事である。
- 必須パーツ（`[Content_Types].xml` / `xl/workbook.xml` 等）が存在しない場合のエラー構築は本ファイルの責務としない。`ZipContainer` は「安全なファイル取得」のみを扱う汎用コンテナ層に徹し、どのパーツが必須かというOPC特有のセマンティクスを持たない（PR #7 レビュー指摘を反映してオープンクエスチョン5を解決）。`get_entry` が返す `Ok(None)` を見て `Error::InvalidPackage` や `Error::DanglingRelationship` のいずれを構築するかは `pipeline.rs` / `parse/relationships.rs` 側の判断とする。
- `get_entry` が返す `BoundedReader` からの読み込み中に上限超過が発生した場合の `io::Error` → `Error::ZipBombDetected` への変換は、`parse/` が `quick_xml::Error` を `crate::error::Error` へ変換する境界で行う（[sanitize.md エラー処理方針](sanitize.md) 参照。オープンクエスチョン3は解決済み）。

## テスト方針

- 正当な最小構成の`.xlsx`相当ZIPを `open` した場合に成功し、`entry_names()` が期待するエントリ集合を返すことの確認
- 破損したZIPバイト列を `open_reader` した場合に `Error::InvalidPackage` を返すことの確認
- 中央ディレクトリに `../evil` のような不正なエントリ名を含むZIPを `open_reader` した場合、`Error::ZipSlipDetected` を返しアーカイブ全体が拒否されることの確認（1件でも不正なら全体拒否）
- `get_entry` に実在するエントリ名を渡した場合に `Ok(Some(..))` を返し、中身のバイト列が期待通りであることの確認
- `get_entry` に実在しないエントリ名を渡した場合に `Ok(None)` を返すことの確認（エラーにならないこと）
- `get_entry` に `"../etc/passwd"` のような不正な形の `name` を渡した場合、アーカイブ内の実在有無に関わらず `Error::ZipSlipDetected` を返すことの確認(オープン時検証をすり抜けた想定の多層防御テスト)
- `has_entry` が実在/不在のエントリ名それぞれに対して `Ok(true)`/`Ok(false)` を返すこと、不正な形の `name` に対しては存在有無に関わらず `Err(Error::ZipSlipDetected)` を返すこと(`get_entry` と同じ多層防御の性質)の確認
- `get_entry` が返すストリームが `max_entry_size` を超えて読まれた場合にエラーとなることの確認（`sanitize::BoundedReader` との結線テスト。`BoundedReader` 自体のロジック検証は [sanitize.md](sanitize.md) 側の責務）
- 複数の `get_entry` 呼び出しにまたがって `total_read` が正しく累積し、`max_total_size` を超えた時点でエラーとなることの確認（`ZipContainer` が `BoundedReader` へ渡す累積カウンタの結線テスト）

## 未決事項 / オープンクエスチョン

1. ~~ZIP操作に使用する外部クレートの選定~~ → **解決**: `zip` クレート（v8）を採用（[error.md オープンクエスチョン1](../error.md) と同一の論点）。`open`/`open_reader` は `zip::result::ZipError` を専用の `#[source]` 保持バリアントにせず、現状は `Error::InvalidPackage(String)` へ文字列化する簡易な受け皿のままとした。呼び出し側がZIP失敗の種類ごとに分岐する必要が出てきたら見直す。
2. ~~`get_entry` の戻り値の型設計~~ → **解決**: `impl Read + '_`（RPIT、`self` の借用に束縛）を採用する。本ライブラリの処理パイプラインは「rels読み込み→SST読み込み→worksheet逐次読み込み」という完全な逐次アクセスパターンであり、複数エントリのストリームを同時に開いておく必要性は設計上存在しない。アロケーションコストがなく、複数ストリームの同時オープン（借用競合）をコンパイル時に防げる `impl Read + '_` の方が `Box<dyn Read + '_>` より望ましい（PR #7 レビュー指摘を反映）。
3. ~~`max_entry_size` / `max_total_size` の設定インターフェース~~ → **解決**: クレート内からのみ呼び出せる `pub(crate)` ビルダーメソッド（`with_max_entry_size` / `with_max_total_size`）として実装した。`pipeline::run` が `lib.rs`（[lib.md](../lib.md)）由来の `SizeLimits` を受け取り、`ZipContainer::open_reader(reader)?.with_max_entry_size(limits.max_entry_size).with_max_total_size(limits.max_total_size)` という形で両ビルダーメソッドを呼び出すようになったことで、公開APIからの可変性（セキュリティレビュー Finding 2）を実現した（Issue [#14](https://github.com/MinamiyamaKotaro/xlsxparser/issues/14)）。
4. ~~アーカイブ全体の累積サイズ追跡~~ → **解決**: `ZipContainer` が `total_read` / `max_total_size` フィールドとして保持する（`ZipContainer` が複数エントリにまたがる状態を持つ自然な置き場所であるため）。`get_entry` が `BoundedReader` へ `&mut u64` として貸し出す設計とし、`Cell` 等の内部可変性は用いない（PR #7 レビュー指摘を反映。詳細は[sanitize.md](sanitize.md)参照）。
5. ~~必須パーツの存在チェックの責務分界~~ → **解決**: `ZipContainer` は「ZIPアーカイブから安全にファイルを切り出す汎用コンテナ層」としての責務に徹し、`.xlsx` (OPC) 特有のセマンティクス（どのパーツが必須か）は持たない（単一責任の原則）。存在チェックは `pipeline.rs` / `parse/relationships.rs` 側が `get_entry` の `Ok(None)` を見てハンドリングする（PR #7 レビュー指摘を反映）。
6. **エントリ名検索の大文字小文字の扱い**: `get_entry` は `zip::ZipArchive::by_name` を使用しており大文字小文字を区別するが、OPCパート名（ECMA-376 Part 2）は仕様上大文字小文字を区別しない（ASCII case folding）。実装時点では、実運用のツール（Excel、Google Sheets、LibreOffice、Apache POI）がエントリ名と `.rels` の `Target` 参照を常にバイト単位で一致させているため、シンプルさを優先しcase-sensitiveのままとした。仕様に非準拠なツールが生成したファイルで問題が生じた場合は、既存の `validate_entry_path` の走査と合わせて `open_reader` 時に `HashMap<String, String>`（小文字化した名前 → 元の名前）を一度だけ構築する対応が考えられる（PR #21 レビュー指摘を反映）。
