# `container/sanitize.rs` 設計書

*[English](sanitize.en.md)*

`src/container/sanitize.rs` に対応する設計書。[architecture.md](../architecture.md) が定義するフェーズ2（サニタイズ）を担う。要求仕様書2章が要求する「Zip Bomb」「Zip Slip（パストラバーサル）」の検知・ブロックのロジックのみを提供する。[error.md](../error.md) には本モジュールの検証失敗に対応するエラーバリアント（`ZipBombDetected` / `ZipSlipDetected`）が既に定義されている。

## 責務・スコープ

- **Zip Slip対策**: ZIPエントリ名がアーカイブのルート外へ脱出しないことを検証する（`validate_entry_path`）
- **Zip Bomb対策**: 展開後バイト数の上限をストリーミングで強制する `Read` ラッパー（`BoundedReader`）を提供する
- Zip Bombサイズ上限、および1シートあたりのセル数上限（`max_cells_per_sheet`。後述）を呼び出し側が指定するための公開設定型 `SizeLimits` を定義する（`lib.rs`（[lib.md](../lib.md)）が再エクスポートし、`parse_workbook_with_limits`/`parse_workbook_reader_with_limits` の引数として使う。セキュリティレビュー Finding 2）
- **セル数上限の値そのものの定義**（Issue [#88](https://github.com/MinamiyamaKotaro/xlsxparser/issues/88)）: `parse::worksheet`（[parse/worksheet.md](../parse/worksheet.md)）が実際に`<c>`をストリーミング中に数えるロジックを持つのに対し、本モジュールは「その上限値をどこに置くか」（`SizeLimits`という同じ公開設定型に、Zip Bomb対策と並べて置く）を決める設計判断のみを担う
- **含まない責務**: ZIPアーカイブそのものの展開・エントリ列挙（`container/mod.rs`）、XMLの構文解釈やXXE対策（`parse/`。要求仕様書2章のXXE要件は architecture.md の議論の経緯により `parse/mod.rs` の責務と確定済み）、セル数を実際にカウントして打ち切るロジック（`parse::worksheet`。本モジュールは上限「値」の置き場所でしかない）

## 主要な型（案）

```rust
use crate::error::Error;
use std::io::{self, Read};

/// フェーズ2のデフォルトの、エントリ単体ごとの展開後サイズ上限（バイト単位）。
/// 呼び出し側（`lib.rs` の公開API）からの上書きは `SizeLimits`
/// （[lib.md](../lib.md)）経由で可能（オープンクエスチョン1で解決）。
pub const DEFAULT_MAX_UNCOMPRESSED_SIZE: u64 = 512 * 1024 * 1024; // 512 MiB

/// フェーズ2のデフォルトの、アーカイブ全体を通じた累積展開後サイズ上限
/// （バイト単位）。中程度のエントリを大量に持つことで累積的にメモリを
/// 圧迫するタイプのZip Bombに対する防御（[container/mod.md](mod.md) 参照。
/// PR #7 レビュー指摘を反映）。
pub const DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB

/// 1シートあたりのデフォルトのセル数上限（Issue #88）。カウント対象は
/// `Sheet::insert_cell` に実際に到達したセルのみ（値・スタイル・共有文字列
/// 参照のいずれも持たない`<c>`は`parse/worksheet.rs`の`flush_cell`が無料で
/// 捨てるため、カウントされない）。実測（`poc/issue88-poc/`、知見はIssue
/// コメントに記録）で、`Sheet::cells`が使う`BTreeMap<CellRef, Cell>`は
/// そうしたセル1件あたり78.3バイト（生の`(CellRef, Cell)`ペア40バイトの
/// 約2倍。差分は`BTreeMap`のノードオーバーヘッド）。上のバイト数上限
/// （`DEFAULT_MAX_UNCOMPRESSED_SIZE`、512MiB）だけではこれを抑えられない
/// ——`<c r="..."><v>1</v></c>`（1セルあたり約26バイト）を敷き詰めた
/// ワークシートXMLは、このバイト数上限内だけで約2,000万セルに達し得て、
/// それが約1.6GBの`Sheet`メモリに増幅する（バイト数上限を約3倍上回る）。
/// 5,000,000という値は、この増幅をバイト数上限とほぼ同じオーダー（約
/// 391MB）まで抑え込みつつ、本クレート自身のテストスイートが実際に使う
/// 最大の正当な規模（`tests/fixtures/load.rs`の`massive_dense_accounting`
/// フィクスチャ、300,000セル）に対して約16倍の余裕を残す。
pub const DEFAULT_MAX_CELLS_PER_SHEET: usize = 5_000_000;

/// Zip Bomb対策のサイズ上限、およびセル数上限を呼び出し側が指定するための
/// 公開設定型。`lib.rs`（[lib.md](../lib.md)）がクレートルートへ再エクス
/// ポートし、`parse_workbook_with_limits`/`parse_workbook_reader_with_limits`
/// の引数として使う。`Default` は `DEFAULT_MAX_UNCOMPRESSED_SIZE` /
/// `DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE` / `DEFAULT_MAX_CELLS_PER_SHEET` を
/// そのまま用いる（値を二重管理せず、`parse_workbook`/`parse_workbook_reader`
/// は内部で `SizeLimits::default()` を渡すだけで済む）。
#[derive(Debug, Clone, Copy)]
pub struct SizeLimits {
    /// 個々のZIPエントリ（シートXML等）の展開後サイズ上限（バイト）。
    /// `ZipContainer::with_max_entry_size`（[container/mod.md](mod.md)）へ
    /// そのまま渡る。
    pub max_entry_size: u64,
    /// アーカイブ全体での累積展開後サイズ上限（バイト）。
    /// `ZipContainer::with_max_total_size`（[container/mod.md](mod.md)）へ
    /// そのまま渡る。
    pub max_total_size: u64,
    /// 1つの`Sheet`に実際に挿入されるセル数の上限（Issue #88）。シート単位
    /// でチェックし、ワークブック全体での累積は見ない——複数シートに分散
    /// させて上限を回避する攻撃までは対象外とする設計判断（各シートが個別
    /// に上限未満なら、合計がどれだけ大きくても受理される。
    /// `resolve::merge::MAX_MERGE_REGIONS`/
    /// `resolve::column_width::MAX_COLUMN_WIDTH_RANGES`と同じくシート単位の
    /// 上限）。`parse::worksheet::parse_worksheet`
    /// （[parse/worksheet.md](../parse/worksheet.md)）へそのまま渡り、
    /// `<c>`をストリーミング中に逐次チェックされる。
    pub max_cells_per_sheet: usize,
}

impl Default for SizeLimits {
    fn default() -> Self {
        Self {
            max_entry_size: DEFAULT_MAX_UNCOMPRESSED_SIZE,
            max_total_size: DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE,
            max_cells_per_sheet: DEFAULT_MAX_CELLS_PER_SHEET,
        }
    }
}

/// ZIPエントリ名がアーカイブのルートより外側へ脱出しないことを検証する
/// （Zip Slip対策）。`container/mod.rs` がアーカイブを開いた直後、
/// 中央ディレクトリの全エントリ名を列挙する時点で本関数を呼び、1件でも
/// 不正なら即座にエラーとする（個々のエントリへの遅延アクセス時に検証するの
/// ではなく、オープン時に一括で検証し「信頼できないエントリ名」がそもそも
/// 後続処理に渡らないようにする）。
///
/// 判定内容:
/// - 空文字列を拒否
/// - `/` 始まりの絶対パスを拒否
/// - バックスラッシュを含むものを拒否（OPC/ZIPの区切り文字として不正。
///   `C:\Windows\System32\evil` のようなWindows形式パスもこれで拒否できる）
/// - Windowsのドライブレタープレフィックス（例: `C:evil`）を上記とは独立に拒否
/// - `/` 区切りの各セグメントに `..`（親ディレクトリ参照）を含むものを拒否
///
/// `std::path::Path` ではなく文字列操作（`starts_with`/`contains`/
/// `split('/')`）で実装する（実装時に確定。PR #7時点の案は
/// `Path::components()` の利用を想定していた）。`Path` のコンポーネント解析は
/// ビルド対象OSごとに条件コンパイルされており（例: バックスラッシュを区切り
/// 文字として扱い、ドライブレターを認識するのは `windows` ターゲットの場合の
/// み）、非Windows環境でビルドすると `C:\Windows\System32\evil` を同様には
/// 拒否できない。本検証はビルド対象OSに関わらず同じ挙動でなければならない。
/// 判定結果は実際のファイルシステムパスとしては解釈・使用しない
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
- `SizeLimits::default()` の `max_entry_size`/`max_total_size`/`max_cells_per_sheet` が、それぞれ `DEFAULT_MAX_UNCOMPRESSED_SIZE`/`DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE`/`DEFAULT_MAX_CELLS_PER_SHEET` と一致することの確認（値の二重管理が実装時にずれていないことの回帰テスト）
- `max_cells_per_sheet` 超過時の実際のカウント・打ち切りロジックのテストは `parse/worksheet.md` 側の責務（本モジュールは値の置き場所でしかないため）

## 未決事項 / オープンクエスチョン

1. ~~サイズ上限のデフォルト値と可変性~~ → **解決**: `DEFAULT_MAX_UNCOMPRESSED_SIZE`（512 MiB）・`DEFAULT_MAX_TOTAL_UNCOMPRESSED_SIZE`（2 GiB）は値として維持する。要求仕様書自体には具体的なファイルサイズ上限の記載がないが、実務上の巨大シート（「方眼紙Excel」、数十万〜100万セル規模）でも展開後XMLサイズは概ね10〜50 MiB程度に収まるため、512 MiBは正当な入力を誤って拒否しない十分な余裕を持ちつつDoSを抑制できる値と判断した。呼び出し側からの上書きは、`lib.rs`（[lib.md](../lib.md)）が新設する `SizeLimits` 構造体と `parse_workbook_with_limits` / `parse_workbook_reader_with_limits` を通じて可能にする（`pipeline::run` が `SizeLimits` を受け取り、[container/mod.md](mod.md) の `with_max_entry_size` / `with_max_total_size` へ橋渡しする。セキュリティレビュー Finding 2、Issue [#14](https://github.com/MinamiyamaKotaro/xlsxparser/issues/14)）。
2. ~~上限のスコープ: エントリ単位か累積か~~ → **解決**: エントリ単位（`per_entry_limit`）に加え、アーカイブ全体の累積展開済みサイズ（`cumulative_limit`）も `BoundedReader` が同時に監視する設計とする。累積カウンタの実体は [container/mod.md](mod.md) の `ZipContainer` がフィールドとして保持し、`get_entry` 呼び出し時に `&mut u64` として `BoundedReader` へ渡す（PR #7 レビュー指摘を反映）。
3. ~~`LimitExceeded` から `Error::ZipBombDetected` への変換層~~ → **解決**: `pipeline.rs` ではなく `parse/` が `quick_xml::Error` を `crate::error::Error` へ変換する境界（型消去する直前）でダウンキャストする。理由・詳細はエラー処理方針セクション参照（PR #7 レビュー指摘を反映。当初検討した「`ZipContainer` に共有フラグ（`Cell`）を持たせ `pipeline.rs` 側で確認する」代替案は、`container/sanitize.rs` の関知しない範囲に恒常的なチェック漏れリスクを生む上、`parse/` 層での変換に比べて余分な内部可変性を要するため採用しない）。
4. **圧縮率ベースの検知の要否**: 現状は展開後の絶対サイズのみで判定するが、ZIP中央ディレクトリから安価に取得できる「宣言された圧縮後サイズ」と「宣言された展開後サイズ」の比率（例: 100倍以上）を用いた早期検知（実際に展開する前段階でのスクリーニング）を `container/mod.rs` 側に追加すべきかは未決定。追加する場合、その判定ロジックを本ファイルに置くか `container/mod.rs` に置くかも未確定。
5. **エントリ名のアローリスト化**: 現状の `validate_entry_path` は「`..` を含まない」等のデナイリスト方式だが、より厳格に「`xl/` `docProps/` `_rels/` `[Content_Types].xml` など既知のOPC名前空間プレフィックスに一致するエントリのみを許可する」アローリスト方式にすべきかは未決定。
6. ~~セル数上限の要否・置き場所・値~~ → **解決**（Issue [#88](https://github.com/MinamiyamaKotaro/xlsxparser/issues/88)）: バイト数上限（`max_entry_size`）だけでは、値を持つ最小限のセル（`<c r="..."><v>1</v></c>`）を敷き詰めることでパース後の `Sheet` メモリを約3倍増幅でき、既存のZip Bomb対策を実質的にすり抜けられることが判明。対応として `SizeLimits` に `max_cells_per_sheet`（デフォルト5,000,000、`poc/issue88-poc/` の実測78.3バイト/セルから逆算）を追加。他パーサーライブラリ（calamine/openpyxl、Issue #88コメント参照）との比較で、この種の脆弱性はexceldiff固有ではなく業界的にも珍しくない設計漏れであることを確認した上での対応。ワークブック累計ではなくシート単位のチェックとする設計判断も含めて確定。
