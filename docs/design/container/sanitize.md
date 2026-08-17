# `container/sanitize.rs` 設計書

*[English](sanitize.en.md)*

`src/container/sanitize.rs` に対応する設計書。[architecture.md](../architecture.md) が定義するフェーズ2（サニタイズ）を担う。要求仕様書2章が要求する「Zip Bomb」「Zip Slip（パストラバーサル）」の検知・ブロックのロジックのみを提供する。[error.md](../error.md) には本モジュールの検証失敗に対応するエラーバリアント（`ZipBombDetected` / `ZipSlipDetected`）が既に定義されている。

## 責務・スコープ

- **Zip Slip対策**: ZIPエントリ名がアーカイブのルート外へ脱出しないことを検証する（`validate_entry_path`）
- **Zip Bomb対策**: 展開後バイト数の上限をストリーミングで強制する `Read` ラッパー（`BoundedReader`）を提供する
- **含まない責務**: ZIPアーカイブそのものの展開・エントリ列挙（`container/mod.rs`）、XMLの構文解釈やXXE対策（`parse/`。要求仕様書2章のXXE要件は architecture.md の議論の経緯により `parse/mod.rs` の責務と確定済み）

## 主要な型（案）

```rust
use crate::error::Error;
use std::io::{self, Read};

/// フェーズ2のデフォルトの、エントリ単体ごとの展開後サイズ上限（バイト単位）。
/// 具体的な値および呼び出し側（`lib.rs` の公開API）からの上書き可否は未決定
/// （オープンクエスチョン1参照）。
pub const DEFAULT_MAX_UNCOMPRESSED_SIZE: u64 = 512 * 1024 * 1024; // 512 MiB（暫定値）

/// フェーズ2のデフォルトの、アーカイブ全体を通じた累積展開後サイズ上限
/// （バイト単位）。中程度のエントリを大量に持つことで累積的にメモリを
/// 圧迫するタイプのZip Bombに対する防御（[container/mod.md](mod.md) 参照。
/// PR #7 レビュー指摘を反映）。
pub const DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB（暫定値）

/// ZIPエントリ名がアーカイブのルートより外側へ脱出しないことを検証する
/// （Zip Slip対策）。`container/mod.rs` がアーカイブを開いた直後、
/// 中央ディレクトリの全エントリ名を列挙する時点で本関数を呼び、1件でも
/// 不正なら即座にエラーとする（個々のエントリへの遅延アクセス時に検証するの
/// ではなく、オープン時に一括で検証し「信頼できないエントリ名」がそもそも
/// 後続処理に渡らないようにする）。
///
/// 判定内容（案）:
/// - 空文字列を拒否
/// - `/` 始まりの絶対パスを拒否
/// - Windowsのドライブレター等（`C:\...`）を拒否
/// - パス構成要素に `..`（親ディレクトリ参照）を含むものを拒否
///
/// 実装は `std::path::Path::components()` を用いて判定するが、これは検証の
/// ためだけに使用し、実際のファイルシステムパスとしては解釈・使用しない
/// （本ライブラリはZIPエントリを実ディスクへ展開しないため、伝統的な
/// Zip Slip被害である「意図しないファイル書き込み」は直接には発生しない。
/// それでもエントリ名を検証する理由は依存関係セクション参照）。
pub fn validate_entry_path(name: &str) -> Result<(), Error>;

/// `BoundedReader::read` が上限超過時に `io::Error` へ埋め込む内部マーカー型。
/// 上限超過を最終的に `Error::ZipBombDetected` へ変換する層（`parse/` が
/// quick-xml のエラーを `crate::error::Error` へ変換する境界。詳細は
/// エラー処理方針参照）が `io::Error::get_ref()` 経由でダウンキャストし
/// `limit` / `actual` を取り出すことを想定する（オープンクエスチョン3は
/// PR #7 レビューを踏まえて解決済み。エラー処理方針参照）。
#[derive(Debug)]
pub(crate) struct LimitExceeded {
    pub limit: u64,
    pub actual: u64,
}

impl std::fmt::Display for LimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "uncompressed size {} bytes exceeds limit {} bytes", self.actual, self.limit)
    }
}
impl std::error::Error for LimitExceeded {}

/// 展開後バイト数の上限を強制する `Read` ラッパー（Zip Bomb対策）。ZIP
/// ヘッダーの自己申告サイズ（"declared uncompressed size"）は偽装されうる
/// ため信頼せず、実際に読み出されたバイト数をストリーミングでカウントし、
/// 上限超過時点で即座にエラーを返す。`container/mod.rs` が各エントリの
/// 展開ストリームを本ラッパーで包んでから `parse/` へ渡す。
///
/// エントリ単体の上限（`per_entry_limit`）に加え、`cumulative_read` へ
/// アーカイブ全体を通じた累積展開済みバイト数を加算し、`cumulative_limit`
/// との比較も行う（[container/mod.md](mod.md) 参照。PR #7 レビュー指摘を
/// 反映しオープンクエスチョン2を解決）。`cumulative_read` は
/// `ZipContainer` が保持するフィールドへの可変参照であり、`Cell` 等の
/// 内部可変性は用いない。`get_entry` が `&mut self` を要求する時点で
/// 既に排他アクセスが保証されているため、`archive` フィールドから得た
/// エントリの読み取りストリームと `cumulative_read` フィールドへの参照を、
/// 同一の `self` から分割借用（disjoint field borrow）するだけで足りる。
pub struct BoundedReader<'a, R> {
    inner: R,
    per_entry_limit: u64,
    per_entry_read: u64,
    cumulative_read: &'a mut u64,
    cumulative_limit: u64,
}

impl<'a, R: Read> BoundedReader<'a, R> {
    pub fn new(
        inner: R,
        per_entry_limit: u64,
        cumulative_read: &'a mut u64,
        cumulative_limit: u64,
    ) -> Self {
        Self { inner, per_entry_limit, per_entry_read: 0, cumulative_read, cumulative_limit }
    }
}

impl<'a, R: Read> Read for BoundedReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.per_entry_read += n as u64;
        *self.cumulative_read += n as u64;
        if self.per_entry_read > self.per_entry_limit {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                LimitExceeded { limit: self.per_entry_limit, actual: self.per_entry_read },
            ));
        }
        if *self.cumulative_read > self.cumulative_limit {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                LimitExceeded { limit: self.cumulative_limit, actual: *self.cumulative_read },
            ));
        }
        Ok(n)
    }
}
```

## 依存関係

- 依存先: [`error.rs`](../error.md)（`Error::ZipSlipDetected` / `Error::ZipBombDetected` を返す）のみ。`model/` を含む他モジュールには依存しない。
- 依存元: `container/mod.rs`（アーカイブオープン時に全エントリ名へ `validate_entry_path` を適用し、各エントリの展開ストリームを `BoundedReader` でラップする）

Zip Slipの検証を「実ディスクへの展開を行わない設計であっても」実施する理由は、エントリ名が後続フェーズで信頼される可能性があるため。具体的には `parse/relationships.rs`（フェーズ1）が `.rels` ファイル内の相対パス（例: `../media/image1.png`）とエントリ名を組み合わせて実体ファイルを解決する際、悪意あるエントリ名や相対パスの組み合わせにより意図しないエントリへ到達しうる。本モジュールがアーカイブオープン時点で全エントリ名をホワイトリスト的に検証しておくことで、後続のどのモジュールも「不正なエントリ名」を扱う可能性を構造的に排除する。

## エラー処理方針

- `validate_entry_path` はパースエラーと同様、`panic` せず `Result<(), Error>` を返す。判定があいまい・解釈できないエントリ名は安全側（拒否）に倒す（fail closed）。
- `BoundedReader::read` は `std::io::Read` トレイトの制約上 `crate::error::Error` を直接返せないため、`io::Error`（内部に `LimitExceeded` を保持）を返す。

  この `io::Error` を最終的に `crate::error::Error::ZipBombDetected` へ変換する境界は、**`pipeline.rs` ではなく `parse/` が quick-xml のエラーを `crate::error::Error` へ変換する箇所（`parse/mod.rs` のセキュアReaderファクトリに併設予定）に置く**（PR #7 レビューを踏まえて確定。旧オープンクエスチョン3を解決）。

  理由: [error.md](../error.md) で確定した設計により `Error::XmlParse::source` は `Box<dyn std::error::Error + Send + Sync + 'static>` として型消去済みである。`pipeline.rs` が受け取るのはこの型消去済みの `Error::XmlParse` のみであり、かつ `pipeline.rs` は `quick-xml` に依存しない（パブリック依存を避けるための設計）ため、`quick_xml::Error` の具体的なバリアント（`Io(io::Error)` 等）へダウンキャストする手段を持たない。一方 `parse/` は元々 `quick-xml` に依存しているため、`quick_xml::Error` を `crate::error::Error` へ変換する自前のタイミングで、まだ型消去される前の `io::Error` を保持していれば `io::Error::get_ref()` → `.downcast_ref::<LimitExceeded>()` が可能である。この変換に成功した場合は `Error::XmlParse` ではなく `Error::ZipBombDetected { limit, actual }` を返す。

  この変換ロジックを全 `parse/*.rs` から共通で呼べる1箇所の関数（例: `parse/mod.rs::convert_xml_error`）に集約することで、変換漏れのリスクを局所化する（詳細な関数シグネチャは `parse/` の設計時に確定）。

## テスト方針

- `validate_entry_path` の拒否ケース: `"../../../etc/passwd"`, `"/etc/passwd"`, `"xl/../../evil"`, `"C:\\Windows\\System32\\evil"`, 空文字列
- `validate_entry_path` の許可ケース: `"xl/worksheets/sheet1.xml"`, `"[Content_Types].xml"`, `"xl/_rels/workbook.xml.rels"`, `"xl/media/image1.png"` など正当なOPCエントリ名
- `BoundedReader`: エントリ単体の上限ちょうど（`per_entry_limit`バイト）までの読み込みが成功することの確認（境界値）
- `BoundedReader`: エントリ単体の上限を1バイトでも超える読み込みが `Err` になり、`LimitExceeded` の `actual`/`limit` が `per_entry_limit` 側の値で正しいことの確認
- `BoundedReader`: エントリ単体は上限内でも、複数エントリにまたがる累積読み込みが `cumulative_limit` を超えた場合に `Err` になり、`LimitExceeded` の `actual`/`limit` が `cumulative_limit` 側の値で正しいことの確認（累積カウンタが呼び出しをまたいで正しく引き継がれることの確認を含む）
- `BoundedReader`: 上限に達する前の通常の読み込みが正しくバイト数をカウント・透過し、`cumulative_read` の値も正しく加算されることの確認

## 未決事項 / オープンクエスチョン

1. **サイズ上限のデフォルト値と可変性**: `DEFAULT_MAX_UNCOMPRESSED_SIZE`（暫定512 MiB）・`DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE`（暫定2 GiB）の具体的な値の妥当性、および `lib.rs` の公開API（`parse_workbook` 等）から呼び出し側が上限を上書きできるようにすべきかは、`lib.rs` の設計時にあわせて確定させる。
2. ~~上限のスコープ: エントリ単位か累積か~~ → **解決**: エントリ単位（`per_entry_limit`）に加え、アーカイブ全体の累積展開済みサイズ（`cumulative_limit`）も `BoundedReader` が同時に監視する設計とする。累積カウンタの実体は [container/mod.md](mod.md) の `ZipContainer` がフィールドとして保持し、`get_entry` 呼び出し時に `&mut u64` として `BoundedReader` へ渡す（PR #7 レビュー指摘を反映）。
3. ~~`LimitExceeded` から `Error::ZipBombDetected` への変換層~~ → **解決**: `pipeline.rs` ではなく `parse/` が `quick_xml::Error` を `crate::error::Error` へ変換する境界（型消去する直前）でダウンキャストする。理由・詳細はエラー処理方針セクション参照（PR #7 レビュー指摘を反映。当初検討した「`ZipContainer` に共有フラグ（`Cell`）を持たせ `pipeline.rs` 側で確認する」代替案は、`container/sanitize.rs` の関知しない範囲に恒常的なチェック漏れリスクを生む上、`parse/` 層での変換に比べて余分な内部可変性を要するため採用しない）。
4. **圧縮率ベースの検知の要否**: 現状は展開後の絶対サイズのみで判定するが、ZIP中央ディレクトリから安価に取得できる「宣言された圧縮後サイズ」と「宣言された展開後サイズ」の比率（例: 100倍以上）を用いた早期検知（実際に展開する前段階でのスクリーニング）を `container/mod.rs` 側に追加すべきかは未決定。追加する場合、その判定ロジックを本ファイルに置くか `container/mod.rs` に置くかも未確定。
5. **エントリ名のアローリスト化**: 現状の `validate_entry_path` は「`..` を含まない」等のデナイリスト方式だが、より厳格に「`xl/` `docProps/` `_rels/` `[Content_Types].xml` など既知のOPC名前空間プレフィックスに一致するエントリのみを許可する」アローリスト方式にすべきかは未決定。
