# `src/` Security Code Review

*[Japanese](code-review.md)*

`docs/security/design-review.md` reviewed the **design docs** before implementation began. This review instead targets the **implementation code under `src/`** on `master` as of 2026-08-17 (after PR #30, the Issue #28 test-fixture work), from a static-analysis (SAST) perspective. Beyond the OWASP Top 10 lens, it also scans for Rust-specific concerns (`unsafe`, panic safety, integer overflow, algorithmic complexity), and every finding below was actually reproduced by running the code, not inferred from reading alone.

## Overall Assessment

The three threats the requirements spec names explicitly — Zip Bomb, Zip Slip, XXE — remain soundly mitigated at the implementation level, there is zero `unsafe` code anywhere in `src/`, and `cargo audit` reported no known vulnerabilities (30 crates scanned, as of 2026-08-17). That said, this review found one implemented-code bug (Finding 1), backed by measurements, that **disproves a design-time assumption that "capping input byte size alone is enough to prevent DoS."** This is exactly the concern `docs/security/design-review.md` and `docs/design/resolve/merge.md` explicitly scoped out and accepted as a risk — the measurements below overturned the premise behind that acceptance, so it was treated as the top priority and fixed, re-verified by measurement, and covered by regression tests immediately after this review (see Finding 1 for details). The missing bounds check on cell coordinates (Finding 2) was fixed the same way. An implicit dependency on a third-party XML parser's default configuration (Finding 3, informational) remains unaddressed.

## Findings

### ~~Finding 1: `<mergeCell>` overlap validation is O(N²), letting a file well under the Zip Bomb byte cap cause CPU-exhaustion DoS~~ → **Resolved**

* **Vulnerability type**: Denial of Service via algorithmic complexity (CWE-407 Algorithmic Complexity / OWASP API4:2023 Unrestricted Resource Consumption)
* **Risk level**: ~~High~~ → Resolved
* **Target**: [`src/resolve/merge.rs`](../../../src/resolve/merge.rs) `resolve()` / `validate_region()`
* **Details (at the time of the finding)**:
  `resolve()` validates the `N` `MergedRegion`s collected from `<mergeCells>` one at a time while accumulating them into `accepted: Vec<MergedRegion>`, but `validate_region` checks the new region against **every region already in `accepted`** on each call (`for other in accepted { if regions_overlap(region, other) ... }`). Each individual overlap check is O(1) (a geometric rectangle-intersection test), but running it N times against a growing accepted list makes the whole sheet's validation O(N²).

  This tradeoff was already known and documented: [`docs/design/resolve/merge.md`](../../design/resolve/merge.md) ("if N gets very large there's room to improve to O(N log N) via sort + sweep-line, but since real-world Excel files rarely reach tens of thousands of merge regions, O(N²) is considered good enough for now") and [`docs/security/design-review.md`](design-review.md) ("concerns about the merge-validation algorithm's complexity (O(N²))... were scoped out of this review, given the Zip Bomb countermeasure already bounds input size by byte count") both accept this risk **on the premise that the Zip Bomb byte cap (512 MiB per entry by default) already keeps N effectively bounded.**

  Measurement shows that premise does not hold. Each `<mergeCell ref="A{i}:B{i}"/>` is only ~20-30 bytes, so the 512 MiB cap alone permits well over 17 million entries in theory. Actual measurements (Apple M2 Pro, release build) scaled with a clean square law:

  | N (count) | Measured time | Compressed file size |
  |---:|---:|---:|
  | 5,000 | 8.4 ms | 22 KB |
  | 10,000 | 29.2 ms | 42 KB |
  | 20,000 | 110.0 ms | 81 KB |
  | 40,000 | 424.3 ms | 158 KB |

  Extrapolating, **around N=194,000 (compressed file size under ~1 MB) already reaches roughly 10 seconds**, and N=1,000,000 (a few MB — nowhere near the 512 MiB cap) causes **several minutes of fully synchronous CPU blocking**. An attacker approaching N=17 million (right up against the byte cap) could make the call effectively never return.

* **Attack scenario (at the time of the finding)**: An attacker submits an otherwise unremarkable-looking `.xlsx` — a few hundred KB to a few MB — that's nothing but a large number of non-overlapping `<mergeCell>` entries, to any upload path that calls this library (document ingestion, data integration, ...). It passes every file-size and Zip Bomb check, so unlike Zip Slip/XXE it produces no immediate error — instead, the thread that called `parse_workbook` (commonly a web server's request handler) blocks for tens of seconds to minutes. Sending a handful of these concurrently is enough to exhaust a thread pool / worker pool.
* **Resolution**: Option A (a defensive cap) was implemented. [`src/resolve/merge.rs`](../../../src/resolve/merge.rs) now defines `pub(crate) const MAX_MERGE_REGIONS: usize = 20_000`, and `resolve()` checks `regions.len() > MAX_MERGE_REGIONS` at the very top, returning the new `Error::TooManyMergedRanges { count, limit }` ([`src/error.rs`](../../../src/error.rs)) if exceeded. This check runs **before** the O(N²) overlap-validation loop, so an over-limit batch now returns in O(N) (just the cost of streaming the XML) regardless of N.

  Re-measuring with the same method after the fix confirms it holds (Apple M2 Pro, release build):

  | N (count) | Before | After |
  |---:|---:|---:|
  | 1,000,000 | minutes (extrapolated) | 260 ms (`Err(TooManyMergedRanges)`) |
  | 5,000,000 | effectively hangs (extrapolated) | 1.32 s (`Err(TooManyMergedRanges)`) |

  Post-fix timing scales with the XML tokenizing cost alone (O(N), already indirectly covered by the Zip Bomb byte cap) — the O(N²) overlap loop no longer runs at all. Regression tests were added: `region_count_at_the_limit_is_accepted` / `region_count_over_the_limit_is_too_many_merged_ranges` in [`src/resolve/merge.rs`](../../../src/resolve/merge.rs) cover the exact N=20,000/20,001 boundary, and `excessive_merge_cell_count_is_too_many_merged_ranges` in [`src/pipeline.rs`](../../../src/pipeline.rs) covers it end to end through real XML.

  The root fix (sort by start row/column and detect overlaps via a sweep-line pass, O(N log N) — already named as a future-improvement path in `docs/design/resolve/merge.md`) was not pursued: 20,000 leaves ample headroom over the tens-to-hundreds of merge cells a real-world sheet typically has, so the defensive cap alone eliminates the actual risk. Making the cap caller-overridable (as `SizeLimits` is) was left for if and when that need arises.

### ~~Finding 2: `CellRef` row/column numbers aren't clamped to Excel's real limits, letting `maxRow`/`maxCol` propagate a downstream resource-exhaustion vector~~ → **Resolved**

* **Vulnerability type**: Missing input validation that propagates downstream (closest to CWE-1284; the library itself doesn't crash, but its trusted output can be used to attack a caller that does)
* **Risk level**: ~~Medium~~ → Resolved
* **Target**: [`src/model/cell.rs`](../../../src/model/cell.rs) `CellRef::from_a1`
* **Details (at the time of the finding)**: `from_a1` only checks that the row number fits in `u32` and isn't `0` — it accepts coordinates far beyond Excel's actual maximum (row 1,048,576; column 16,384 = `XFD`) as a valid `CellRef`. Feeding a few-hundred-byte `.xlsx` containing `<c r="ZZZZZZ4294967294">` through the library produced this (measured, `xlsxparser` itself completes normally):

  ```json
  {"sheets":[{"name":"Sheet1","visibility":"visible","maxRow":4294967294,"maxCol":321272406,"cells":[{"row":4294967294,"col":321272406,"value":{"type":"number","value":1.0}}]}]}
  ```

  `xlsxparser` itself is unaffected — it stores exactly one entry in its coordinate-keyed `HashMap`, the same design property demonstrated as an advantage in README.md's Benchmarks section. But `maxRow`/`maxCol` are passed straight through to the caller as "the sheet's bounding box." Any frontend or downstream service that trusts those numbers to allocate a dense array/grid can be driven into the same kind of OOM/process-kill this review directly observed happening to `calamine` (see README.md "Benchmarks").
* **Attack scenario (at the time of the finding)**: An attacker uploads a `.xlsx` with a forged coordinate. `xlsxparser` parses it fine and returns JSON, but a frontend (rendering a spreadsheet UI) or an Excel re-export feature that trusts `maxRow`/`maxCol` attempts to allocate an array sized to the reported (nonexistent) row/column count, and crashes or exhausts memory.
* **Resolution**: implemented as proposed. `CellRef` in [`src/model/cell.rs`](../../../src/model/cell.rs) now defines `pub const MAX_ROW: u32 = 1_048_576` / `pub const MAX_COL: u32 = 16_384`, and `from_a1` checks `row > Self::MAX_ROW || col > Self::MAX_COL` right alongside its existing `row == 0` check, returning the existing `Error::InvalidCellRef` when exceeded (no new error variant needed).

  `"XFD1048576"` (exactly at the boundary — Excel's actual maximum cell) was confirmed to still succeed via the existing `from_a1_to_a1_round_trip` test. Added `"A1048577"` (row one over) and `"XFE1"` (column one over) to `from_a1_rejects_invalid_strings`, a dedicated regression test `from_a1_rejects_row_or_col_far_beyond_excels_real_maximum` exercising the exact coordinate this finding measured, and an end-to-end regression test through real worksheet XML, `cell_ref_beyond_excels_real_maximum_is_invalid_cell_ref` ([`src/pipeline.rs`](../../../src/pipeline.rs)).

### Finding 3 (informational): rich-text depth counter implicitly depends on a third-party XML parser's default configuration

* **Vulnerability type**: Reliance on an implicit assumption (the same pattern `docs/security/design-review.md` Finding 1 already flagged and fixed once, in a different module)
* **Risk level**: Low (currently unreachable in the existing code; a defense-in-depth suggestion for future changes)
* **Target**: [`src/parse/mod.rs`](../../../src/parse/mod.rs) `concat_rich_text`'s `skip_depth: u32`
* **Details**: `concat_rich_text` increments/decrements `skip_depth` (a `u32`) on `<rPr>`/`<rPh>` start/end tags. If a `</rPr>` ever appeared without a matching `<rPr>`, `skip_depth -= 1` would run while `skip_depth == 0`, panicking in debug builds and wrapping to `u32::MAX` in release.

  Verified this path is **unreachable in the code as it stands today**: the `quick_xml::Reader` `create_secure_reader` builds never changes `check_end_names` (confirmed default `true` in `quick-xml 0.41.0`'s `Config::default()`), so a mismatched end tag is rejected as an XML syntax error by `read_event` before `concat_rich_text` ever sees it as an `Event::End`.
* **Risk scenario**: Not exploitable today. But this safety property is recorded nowhere in `concat_rich_text`'s own code — it rests entirely on a default setting of an external crate, exactly the same shape of risk `docs/security/design-review.md` Finding 1 once flagged explicitly (there, for XXE mitigation) and fixed by introducing `read_event` as an explicit safeguard. If `check_end_names` is ever disabled, or the parser is swapped, this implicit assumption alone would silently stop holding and the DoS would reopen.
* **Recommended fix**: change `skip_depth -= 1` to `skip_depth = skip_depth.saturating_sub(1)` as a defense-in-depth measure (effectively free), and add a code comment noting the dependency on `check_end_names`'s default.

## What held up well

* Zero `unsafe` code anywhere in `src/`.
* `cargo audit` (30 crates scanned, 2026-08-17) reported no known vulnerabilities.
* Zip Bomb (`container/sanitize.rs::BoundedReader`, counting real bytes read rather than trusting the ZIP header's declared size), Zip Slip (`validate_entry_path`, checked both at archive-open time and again on every dynamically-resolved path), and XXE (`read_event`, rejecting a DOCTYPE declaration's mere presence unconditionally) all function soundly at the implementation level exactly as designed, backed by regression tests using real attack payloads (`tests/real_error.rs`, `tests/security.rs`, etc.).
* Outside of Findings 1-2, `u32` arithmetic overflow was checked individually; `MergedRegion::row_span`/`col_span`'s subtraction cannot overflow even at the theoretical maximum, thanks to the invariant that `CellRef`'s row/col are always `>= 1`.
* Production code has exactly 2 `.expect()` calls (`resolve/style.rs`, `resolve/shared_strings.rs`), both resting on an invariant maintained entirely within `parse/worksheet.rs` (a `Pending*` record and its corresponding cell insertion always happen together) — and `Sheet::insert_cell`/`insert_merge` were confirmed to have no code path that ever removes a cell, so neither is reachable from malicious file content.
