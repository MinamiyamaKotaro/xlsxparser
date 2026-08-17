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

/// フェーズ2のデフォルトの展開後サイズ上限（バイト単位）。具体的な値および
/// 呼び出し側（`lib.rs` の公開API）からの上書き可否は未決定（オープンクエスチョン1参照）。
pub const DEFAULT_MAX_UNCOMPRESSED_SIZE: u64 = 512 * 1024 * 1024; // 512 MiB（暫定値）

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
/// 上限超過を最終的に `Error::ZipBombDetected` へ変換する層（`container/mod.rs`、
/// または `parse/` がI/Oエラーを検知する箇所）が `io::Error::into_inner()`
/// 経由でダウンキャストし `limit` / `actual` を取り出すことを想定する
/// （オープンクエスチョン3参照）。
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
pub struct BoundedReader<R> {
    inner: R,
    limit: u64,
    read_so_far: u64,
}

impl<R: Read> BoundedReader<R> {
    pub fn new(inner: R, limit: u64) -> Self {
        Self { inner, limit, read_so_far: 0 }
    }
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read_so_far += n as u64;
        if self.read_so_far > self.limit {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                LimitExceeded { limit: self.limit, actual: self.read_so_far },
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
- `BoundedReader::read` は `std::io::Read` トレイトの制約上 `crate::error::Error` を直接返せないため、`io::Error`（内部に `LimitExceeded` を保持）を返す。呼び出し側で `crate::error::Error::ZipBombDetected` へ変換する境界の設計は未確定（オープンクエスチョン3参照）。

## テスト方針

- `validate_entry_path` の拒否ケース: `"../../../etc/passwd"`, `"/etc/passwd"`, `"xl/../../evil"`, `"C:\\Windows\\System32\\evil"`, 空文字列
- `validate_entry_path` の許可ケース: `"xl/worksheets/sheet1.xml"`, `"[Content_Types].xml"`, `"xl/_rels/workbook.xml.rels"`, `"xl/media/image1.png"` など正当なOPCエントリ名
- `BoundedReader`: 上限ちょうど（`limit`バイト）までの読み込みが成功することの確認（境界値）
- `BoundedReader`: 上限を1バイトでも超える読み込みが `Err` になり、`LimitExceeded` の `actual`/`limit` が正しいことの確認
- `BoundedReader`: 上限に達する前の通常の読み込みが正しくバイト数をカウント・透過することの確認

## 未決事項 / オープンクエスチョン

1. **サイズ上限のデフォルト値と可変性**: `DEFAULT_MAX_UNCOMPRESSED_SIZE` の具体的な値（暫定512 MiB）の妥当性、および `lib.rs` の公開API（`parse_workbook` 等）から呼び出し側が上限を上書きできるようにすべきかは、`lib.rs` の設計時にあわせて確定させる。
2. **上限のスコープ: エントリ単位か累積か**: 現状の `BoundedReader` は1エントリ（1ファイル）ごとの上限のみを強制する。Zip Bombは単一の極端な圧縮率のエントリだけでなく、中程度のエントリを大量に持つアーカイブ（累積で膨大になるケース）でも成立しうるため、`container/mod.rs` 側でアーカイブ全体の累積展開済みサイズも別途追跡・制限するかは未決定。
3. **`LimitExceeded` から `Error::ZipBombDetected` への変換層**: `BoundedReader` は `parse/` が保有する `quick-xml` の `Reader` に渡されることが想定されるため、上限超過時の `io::Error` は `quick-xml` 経由でさらにラップされた状態で伝播する可能性が高い。この場合、`error.md` で確定した `XmlParse::source`（`Box<dyn Error>` に型消去済み）として扱われてしまい、`Error::ZipBombDetected` の持つ構造化情報（`limit`/`actual`）が失われる恐れがある。`io::Error::into_inner()` を辿って `LimitExceeded` へダウンキャストし、`XmlParse` ではなく `ZipBombDetected` として再構築する変換ロジックをどの層（`parse/mod.rs`のセキュアReaderファクトリ、または`pipeline.rs`）に置くかは、`parse/` の設計時にあわせて確定させる。
4. **圧縮率ベースの検知の要否**: 現状は展開後の絶対サイズのみで判定するが、ZIP中央ディレクトリから安価に取得できる「宣言された圧縮後サイズ」と「宣言された展開後サイズ」の比率（例: 100倍以上）を用いた早期検知（実際に展開する前段階でのスクリーニング）を `container/mod.rs` 側に追加すべきかは未決定。追加する場合、その判定ロジックを本ファイルに置くか `container/mod.rs` に置くかも未確定。
5. **エントリ名のアローリスト化**: 現状の `validate_entry_path` は「`..` を含まない」等のデナイリスト方式だが、より厳格に「`xl/` `docProps/` `_rels/` `[Content_Types].xml` など既知のOPC名前空間プレフィックスに一致するエントリのみを許可する」アローリスト方式にすべきかは未決定。
