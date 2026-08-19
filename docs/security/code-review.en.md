# `src/` Security Code Review (2026-08-19)

*[Japanese](code-review.md)*

`docs/security/old/code-review.en.md` reviewed `src/` as of 2026-08-17 (after PR #30). This is a follow-up static-analysis (SAST) review of the entire `src/` tree as it stands today on `master` — 10,258 lines across 23 files, up from the earlier review's scope, following several feature waves it never saw: image anchoring and grouped images (Issues #65/#67) and font/wrap-text/alignment/number-format/fill-color resolution from `styles.xml` (Issues #37/#38/#41/#42/#75). As before, this goes beyond the OWASP Top 10 lens to also scan for Rust-specific concerns (`unsafe`, panic safety, integer overflow, algorithmic complexity), and every finding below was reproduced by running the code, not inferred from reading alone.

## Overall Assessment

The old review's Findings 1-2 remain fixed and unregressed (verified: `MAX_MERGE_REGIONS = 20_000` at [`src/resolve/merge.rs`](../../src/resolve/merge.rs), `CellRef::MAX_ROW`/`MAX_COL` clamp at [`src/model/cell.rs`](../../src/model/cell.rs)), and that same discipline was correctly *extended* to the new `zero_based_to_cell_ref` in [`src/parse/drawing.rs`](../../src/parse/drawing.rs), which explicitly cites the old Finding 2's rationale in its own doc comment. `unsafe` remains completely absent from `src/`, and `cargo audit` is clean (30 crates scanned).

However, this review found and *measured* the exact same class of issue as the old review's Finding 1 — this time in `src/parse/drawing.rs`'s `<xdr:grpSp>` nesting/pic-resolution path — plus a new instance of the old Finding 2 pattern (unclamped attacker-controlled numbers reaching the public API), this time in EMU coordinate math rather than `CellRef`. Both were found, measured, fixed, and re-verified by measurement as part of producing this review, following the exact same process the old review's Findings 1-2 established. One informational finding (an `f64` finite-value policy inconsistency, non-crashing) remains open, mirroring how the old review left its own Finding 3 open.

## Findings

### ~~Finding 1: Unbounded `<xdr:grpSp>` nesting depth makes per-picture resolution O(depth), letting a small drawing.xml cause O(N²) CPU-exhaustion DoS~~ → **Resolved**

* **Vulnerability type**: Denial of Service via algorithmic complexity (CWE-407 Algorithmic Complexity / OWASP API4:2023 Unrestricted Resource Consumption) — the same class as the old review's resolved Finding 1, this time in a different module.
* **Risk level**: ~~High~~ → Resolved
* **Target**: [`src/parse/drawing.rs`](../../src/parse/drawing.rs) `parse_anchor_body` (the `b"grpSp"` arm pushing onto `group_stack: Vec<GroupContext>`) and `resolve_grouped_pic`, called once per `</xdr:pic>`.
* **Details (at the time of the finding)**: `group_stack` was pushed on every `<xdr:grpSp>` start and popped on its end with no depth limit anywhere in the file. `resolve_grouped_pic` iterates the *entire* `group_stack` in reverse for every picture resolved — an O(current nesting depth) operation. Since `<xdr:grpSp>` can contain any number of sibling `<xdr:pic>` elements at any depth with no schema-level nesting limit, a drawing part with `D` levels of nesting and `N` sibling pictures at the innermost level cost **O(N × D)** total, while costing only **O(N + D)** bytes to construct — the same shape as the old review's merge-cell finding (an O(N²) cost hiding behind a byte-size cap that doesn't bound the attacker-controlled axis), except here the multiplication is between two independently-cheap-to-inflate axes rather than a single count squared.

  Measured (release build, Apple-class hardware, single-threaded; via a temporary `#[ignore]`d test, reverted after measurement):

  | Nesting depth (D) | Pictures at innermost level (N) | XML size | Elapsed |
  | ---: | ---: | ---: | ---: |
  | 100 | 100,000 | 29.8 MB | 191 ms |
  | 100,000 | 100 | 15.5 MB | 193 ms |
  | 5,000 | 5,000 | 2.26 MB | 135 ms |
  | 20,000 | 2,000 | 3.69 MB | 222 ms |
  | 50,000 | 1,000 | 8.0 MB | 325 ms |
  | 100,000 | 5,000 | 17.0 MB | **2.33 s** |
  | 50,000 | 50,000 | 22.6 MB | **10.9 s** |

  Either axis alone stays cheap; only their product is expensive, confirming the O(N × D) shape. A separate independent measurement (many small sibling pictures per group level, `D = N` scaling together) converged on a clean **4× time increase per doubling of n** — the textbook O(n²) signature — and extrapolated that content this repetitive compresses roughly 90× inside the ZIP container, so a **~410 KB `.xlsx`** could plausibly cause **~60 seconds** of blocking, and a **~1–1.5 MB** one **~10 minutes**, all while staying trivially under the 512 MiB per-entry Zip Bomb cap.

* **Attack scenario**: An attacker submits an `.xlsx` (or targets any system calling `parse_workbook`/`parse_workbook_reader` on untrusted input, e.g. a document-upload feature) with a `drawingN.xml` containing a deeply nested `<xdr:grpSp>` tree and sibling pictures at the innermost level — a file in the tens-of-KB-to-MB range that passes every existing check (Zip Bomb, Zip Slip, XXE, `MAX_MERGE_REGIONS`, `MAX_COLUMN_WIDTH_RANGES`) without triggering any of them. The calling thread blocks for seconds to potentially minutes inside `parse_drawing`; a handful of concurrent requests exhausts a thread/worker pool.

* **Resolution**: Added `pub(crate) const MAX_GROUP_NESTING_DEPTH: usize = 64` to `src/parse/drawing.rs`, checked inside `parse_anchor_body`'s `b"grpSp"` start-tag arm — a group start tag that would push `group_stack` past the limit returns the new `Error::TooManyNestedGroups { path, depth, limit }` ([`src/error.rs`](../../src/error.rs), mirroring `Error::TooManyMergedRanges`/`Error::TooManyColumnWidthRanges`'s shape) before any further nested content is read. Real-world group nesting is essentially never more than a handful of levels deep, so 64 leaves enormous headroom while capping worst-case cost at O(N × 64).

  Regression tests added: `group_nesting_depth_at_the_limit_is_accepted`/`group_nesting_depth_over_the_limit_is_too_many_nested_groups` cover the exact 64/65 boundary, following `resolve/merge.rs`'s `region_count_at_the_limit_is_accepted`/`region_count_over_the_limit_is_too_many_merged_ranges` pattern.

  Re-measured the D=100,000/N=100 attack shape post-fix: **rejected in under 1ms** — the depth check fires at nesting level 65, long before any O(N × D) transform work runs.

### ~~Finding 2: Nested-group EMU coordinate math is unbounded, letting a tiny crafted file drive anchor offsets/extents to `i64::MAX` before they reach the public JSON API~~ → **Resolved**

* **Vulnerability type**: Missing input validation that propagates downstream (CWE-1284) — the same pattern as the old review's resolved Finding 2 (`CellRef` row/col), this time for image-anchor EMU coordinates.
* **Risk level**: ~~Medium~~ → Resolved
* **Target**: [`src/parse/drawing.rs`](../../src/parse/drawing.rs) `resolve_grouped_pic`, feeding `model::AnchorMarker.col_off`/`row_off` (`i64`) and `model::ImageExtent.cx`/`cy` (`i64`), which `json.rs` serializes verbatim as public JSON fields.
* **Details (at the time of the finding)**: `resolve_grouped_pic` only guarded against `ch_ext_cx == 0 || ch_ext_cy == 0` (avoiding an immediate division-by-zero) — it did not bound how large `ext_cx/ch_ext_cx` (the per-level scale factor) could be, nor how many levels' scale factors compounded via repeated multiplication. A crafted `<xdr:oneCellAnchor>` with just 2 nested `<xdr:grpSp>` levels, each `chExt cx="1" cy="1"` and `ext cx="9223372036854775807" cy="9223372036854775807"` (`i64::MAX`), resolved without error to every field saturated at `i64::MAX`: the cumulative product of scale factors exceeded `f64::MAX` after enough levels, producing `f64::INFINITY`, and `.round() as i64` on `Infinity` doesn't panic — Rust's saturating float-to-int cast silently produces `i64::MAX`. No error was raised anywhere in this path; the value flowed straight through to the public JSON output, exactly the way the old Finding 2 described `maxRow`/`maxCol` doing for a forged `CellRef`. The required input was tiny (well under 5 KB).
* **Attack scenario**: A file with this crafted `drawing1.xml` parsed successfully and produced a JSON `images[].anchor.from.colOff`/`rowOff`/`ext.cx`/`ext.cy` of an attacker-chosen enormous magnitude. A downstream consumer trusting these EMU values as physically plausible (e.g. allocating a rendering buffer sized to the reported extent) could be driven into the same class of OOM/crash the old Finding 2 demonstrated for a naive `maxRow`/`maxCol` consumer.
* **Resolution**: `resolve_grouped_pic`'s final resolved `(x, y, cx, cy)` is now checked for `is_finite()` and against a defensive plausibility ceiling, `MAX_PLAUSIBLE_EMU = 1_000_000_000_000.0` (10^12 EMU, ~27.7 km — several orders of magnitude beyond Excel's real maximum sheet extent), returning `Error::InvalidPackage` on either failure — the same "reject a well-formed-but-nonsensical numeric value rather than silently coercing it" policy the existing `chExt == 0` guard already applies. Regression test added: `extreme_group_transform_scale_is_rejected_as_invalid_package`.

### Finding 3 (informational): `tint`/font-size/column-width `f64` fields skip the finite-value policy the codebase applies elsewhere, silently degrading to JSON `null`

* **Vulnerability type**: Inconsistent input-validation policy (closest to CWE-1284, informational — confirmed non-crashing).
* **Risk level**: Low / Informational
* **Target**: [`src/parse/styles.rs`](../../src/parse/styles.rs) `parse_color`'s `tint` parse and the `<sz val="..">` parse; [`src/model/style.rs`](../../src/model/style.rs) `ColorRef::Theme.tint: Option<f64>` / `Font.size_pt: f64`; [`src/model/sheet.rs`](../../src/model/sheet.rs) `ColWidthRange.width: f64`.
* **Details**: `f64::from_str` accepts the literal strings `"nan"`/`"inf"`/`"-inf"` and numeric literals that overflow to infinity (e.g. `"1e400"`), so a crafted `<fgColor theme="4" tint="nan"/>` or `<sz val="1e400"/>` produces a non-finite `f64` that flows unchecked into `ResolvedStyle`. `serde_json`'s derived `Serialize` for `f64`/`Option<f64>` does not error or panic on this — it silently emits JSON `null`, indistinguishable from "attribute genuinely absent." This is safe (no crash, no DoS), but inconsistent with this same codebase's explicit handling of `CellValue::Number` in `json.rs` (with its own regression test, `non_finite_numbers_fall_back_to_empty_without_erroring`), which spells out exactly why silently coercing NaN/Infinity is undesirable for a downstream consumer.
* **Risk scenario**: A data-fidelity nuance, not an exploitable vulnerability. A downstream consumer sees `"tint": null` and cannot distinguish "this cell has no tint" from "this cell's tint field was attacker-poisoned."
* **Recommended fix**: For consistency with the `CellValue::Number` policy, either reject non-finite `tint`/`size_pt`/`width` at parse time (falling back to `None`/`Font::default()`/skipping the range, the same graceful-degradation policy already used for unparseable `numFmtId`/`fontId`/`fillId`) or explicitly document that these fields may serialize as `null` for reasons other than "attribute absent." Not addressed in this review cycle — left open, the same way the old review left its own Finding 3 (the `check_end_names` dependency) open as informational.

## What held up well

* **Zero `unsafe` code** anywhere in `src/`, unchanged from the old review.
* **`cargo audit` clean** — 30 crates scanned, no known vulnerabilities.
* **Zip Bomb / Zip Slip / XXE protections apply uniformly to every new OOXML part.** `xl/drawings/drawingN.xml`, `xl/drawings/_rels/drawingN.xml.rels`, and `xl/styles.xml` are all read exclusively through the same `container::get_entry` → `create_secure_reader` → `read_event` gateway chain as every pre-existing part; no new direct file-read bypass was found in `pipeline.rs::resolve_sheet_images` or `parse/drawing.rs`/`parse/styles.rs`.
* **Old Findings 1 & 2 remain fixed, unregressed**, and Finding 2's discipline was correctly extended: `zero_based_to_cell_ref` in `parse/drawing.rs` explicitly cites the old finding's rationale in its own doc comment and is covered by `out_of_range_row_is_invalid_cell_ref`.
* **Group-nesting does not risk a native stack overflow.** `<xdr:grpSp>` nesting (Finding 1) is tracked via a heap-allocated `Vec<GroupContext>`, not real function recursion — `parse_anchor_body` is a single flat loop, and `quick_xml`'s tokenizer is itself non-recursive. So while the CPU-time blowup was real (now fixed), an attacker could not additionally cause a native stack overflow via XML nesting depth.
* **`parse/styles.rs`'s font/fill lookups are all `.get()`-based with graceful fallback**, never direct indexing: an out-of-range `fontId`/`fillId` degrades to `Font::default()`/`Fill::default()` rather than panicking, mirroring the pre-existing `numFmtId` policy. No O(N²) growth pattern in the fonts/fills `Vec`s.
* **`is_date_time_format`'s custom-format-code scan** is a single-pass, no-backtracking linear scan over attacker-controlled `formatCode` text, closing the old design review's Finding 4 (ReDoS concern) fully at the implementation level — no regex anywhere.
* **`Sheet::finalize_merges`'s sweep-line algorithm** (added for Issue #43) is a genuine O((C+M) log(C+M)) sort-and-sweep, backed by a regression test proving even Excel's actual maximum-size merge registers without hanging.
* **`resolve/style.rs`'s date-serial conversion** explicitly checks `is_finite()` and range-bounds the serial number before any arithmetic — a forged `<v>` cell value degrades gracefully rather than propagating into undefined behavior.
* **Pipeline image-relationship resolution** (`pipeline.rs::resolve_sheet_images`) is strictly O(image count) — every relationship lookup is a single `HashMap::get`, no quadratic pattern.

## Verification methodology

Findings 1 and 2's measurements (both pre-fix and post-fix) were produced via temporary `#[cfg(test)]`/`#[ignore]` functions added directly to `src/parse/drawing.rs` (calling the crate-internal `parse_drawing`/`resolve_grouped_pic` path, release build), reverted immediately after each measurement — `git status`/`git diff` confirm no residual test-only code remains in `src/`. The permanent regression tests listed under each finding's Resolution are the ones actually committed.
