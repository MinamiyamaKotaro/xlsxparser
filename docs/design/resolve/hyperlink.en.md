# `resolve/hyperlink.rs` Design Doc

*[日本語](hyperlink.md)*

Design doc for `src/resolve/hyperlink.rs`. Handles the "deferred resolution of cell hyperlink ranges" part of Phase 4 as defined by [architecture.md](../architecture.en.md) (Issue #95). Validates the `<hyperlinks>` range list `pipeline.rs` resolved (`r:id` → raw Target string, done there since it needs ZIP I/O) before registering it with [`model::Sheet::finalize_hyperlinks`](../model/sheet.en.md).

Deliberately mirrors [`resolve/merge.rs`](merge.en.md)'s shape almost exactly: both are "a batch of rectangular ranges, keyed by nothing durable, that must be resolved against already-populated cells without expanding into one entry per covered cell." The overlap-validation-then-sweep-line two-step this file follows is `resolve/merge.rs` + `Sheet::finalize_merges`'s proven approach, reused rather than reinvented.

## Responsibility / Scope

- Takes the hyperlink range list (`Vec<model::sheet::HyperlinkRange>`) `pipeline.rs` built (Phase 3.5: `<hyperlink ref>` parsed by `parse/worksheet.rs`, `r:id` resolved against the worksheet's own `_rels` by `pipeline.rs`), validates each range, and calls `Sheet::finalize_hyperlinks` once with the whole batch
- Performs the pre-validation `Sheet::finalize_hyperlinks`'s sweep-line resolution depends on: rejecting a reversed start/end pair, and rejecting any two ranges that overlap each other
- Enforces `MAX_HYPERLINKS_PER_SHEET`, the same class of amplification guard `resolve::merge::MAX_MERGE_REGIONS` already provides, before the O(N²) overlap check runs
- **Not responsible for**: parsing `<hyperlink ref="...">` XML into a `CellRef` pair (`parse/worksheet.rs`); resolving `r:id` against `_rels` into a raw Target string (`pipeline.rs`, since it needs ZIP I/O — see `architecture.md` design principle 2, which this file's own I/O-independence must not compromise); the sweep-line resolution algorithm itself (`model::Sheet::finalize_hyperlinks` — see [model/sheet.en.md](../model/sheet.en.md))

## Key Types / Functions (draft)

```rust
use crate::error::Error;
use crate::model::sheet::{HyperlinkRange, Sheet};

/// Cap on the number of `<hyperlink>` entries accepted for a single sheet.
/// Deliberately reuses `resolve::merge::MAX_MERGE_REGIONS`'s exact value
/// (20,000) and reasoning rather than deriving an independent one:
/// `validate_range` below is the same O(N²) shape as `resolve::merge`'s
/// `validate_region` (each new range checked against every already-
/// accepted one), so the identical cost curve applies — the same
/// measurement `resolve::merge`'s Open Question 2 addendum recorded
/// (~424ms at N=40,000, ~10s extrapolated at N=194,000) governs this cap
/// too, without needing to re-derive it.
pub(crate) const MAX_HYPERLINKS_PER_SHEET: usize = 20_000;

/// Validates `ranges`, then registers the whole batch into `sheet` in one
/// call to `Sheet::finalize_hyperlinks`. Unlike `resolve::merge::resolve`
/// (which calls `Sheet::insert_merge` once per region, then
/// `Sheet::finalize_merges` once at the end), there is no per-range
/// `Sheet` call here — `finalize_hyperlinks` both backfills each range's
/// placeholder cell and runs the sweep in one pass, since (unlike merge
/// registration) there is no reason to expose the pre-sweep state to any
/// other caller.
pub(crate) fn resolve(sheet: &mut Sheet, ranges: Vec<HyperlinkRange>) -> Result<(), Error> {
    if ranges.len() > MAX_HYPERLINKS_PER_SHEET {
        return Err(Error::TooManyHyperlinks {
            count: ranges.len(),
            limit: MAX_HYPERLINKS_PER_SHEET,
        });
    }
    let mut accepted: Vec<&HyperlinkRange> = Vec::with_capacity(ranges.len());
    for range in &ranges {
        validate_range(range, &accepted)?;
        accepted.push(range);
    }
    sheet.finalize_hyperlinks(ranges);
    Ok(())
}

/// Validates a single hyperlink range's start/end ordering and its
/// disjointness from every range already validated. Overlap detection is
/// the same O(1)-per-pair separating-axis test `resolve::merge`'s
/// `regions_overlap` uses — never expanded into per-cell comparisons, so
/// even a huge range (`A1:XFD1048576`) costs the same as a 1x1 one.
///
/// Only hyperlink-range-vs-hyperlink-range overlap is checked. A
/// hyperlink range overlapping a `MergedRegion` is fine and expected —
/// merges and hyperlinks are independent OOXML concepts occupying the
/// same coordinate space, and nothing here needs them mutually exclusive.
fn validate_range(range: &HyperlinkRange, accepted: &[&HyperlinkRange]) -> Result<(), Error> {
    if range.start.row > range.end.row || range.start.col > range.end.col {
        return Err(Error::InvalidHyperlinkRange {
            start: range.start.to_a1(),
            end: range.end.to_a1(),
            reason: "start must not be greater than end".to_string(),
        });
    }
    for other in accepted {
        if ranges_overlap(range, other) {
            return Err(Error::InvalidHyperlinkRange {
                start: range.start.to_a1(),
                end: range.end.to_a1(),
                // Names the conflicting range's own coordinates (unlike
                // resolve::merge's equivalent message), since a hyperlink
                // range doesn't get the extra debugging context a merge's
                // visible cell layout already gives a reader for free —
                // Copilot PR review, PR #96.
                reason: format!(
                    "overlaps with another hyperlink range ({}:{})",
                    other.start.to_a1(),
                    other.end.to_a1()
                ),
            });
        }
    }
    Ok(())
}

fn ranges_overlap(a: &HyperlinkRange, b: &HyperlinkRange) -> bool {
    a.start.row <= b.end.row
        && a.end.row >= b.start.row
        && a.start.col <= b.end.col
        && a.end.col >= b.start.col
}
```

## Dependencies

- Depends on: [`model/sheet.rs`](../model/sheet.en.md) (`Sheet::finalize_hyperlinks`, `HyperlinkRange`), [`error.rs`](../error.en.md)
- Depended on by: `pipeline.rs` (called directly from `run`'s per-sheet loop, Phase 3.5 — not threaded through `resolve::resolve_sheet`, since building the `Vec<HyperlinkRange>` batch itself needs ZIP I/O for `r:id` resolution, which must happen before this I/O-independent function can run at all)

## Error Handling Policy

- A reversed start/end pair, or a range overlapping another hyperlink range, is rejected as `Error::InvalidHyperlinkRange { start, end, reason }` — mirrors `Error::InvalidMergedRange` exactly.
- Overlap is rejected outright rather than resolved via a tie-break policy (e.g. "last one wins"), because `Sheet::finalize_hyperlinks`'s sweep-line resolution depends on active ranges at any given row having disjoint column spans to find the (unique) covering range via a single binary search per query cell — the same precondition `Sheet::finalize_merges` already relies on. Without that guarantee, the sweep's column-sorted-active-set shortcut becomes unsound: a query cell nested inside one active range but past a second, later-starting one could silently resolve to the wrong range, or none at all, rather than a well-defined "which one wins" outcome. Real-world files essentially never declare overlapping hyperlink ranges — Excel's own UI has no path to create one, unlike, say, two independently-authored merges that a corrupted file might combine — so rejecting the malformed case outright, the same choice already made for overlapping `<mergeCell>` ranges, was preferred over adding tie-break logic for a case not expected to occur legitimately.
- `MAX_HYPERLINKS_PER_SHEET` is checked before the O(N²) overlap loop ever runs, the same "reject the batch size before doing the expensive work" order `resolve::merge::resolve` already follows.
- No `panic`: an invalid hyperlink range can stem from untrusted external input (a malformed `.xlsx`).
- Once validation fails, `resolve` aborts entirely and no ranges are registered (reject the whole batch if even one is invalid — same fail-closed principle as `resolve::merge::resolve`).

## Testing Strategy

- Verify that multiple non-overlapping hyperlink ranges are correctly registered (a wiring test confirming `Sheet::hyperlink_at` resolves every covered cell, not just each range's origin)
- Verify that a range with reversed start/end coordinates returns `Error::InvalidHyperlinkRange`
- Verify that two hyperlink ranges overlapping even partially return `Error::InvalidHyperlinkRange`
- Verify that two ranges merely adjacent (never actually overlapping) are not mistakenly flagged (boundary-value test for `ranges_overlap`, identical in shape to `resolve::merge`'s equivalent)
- Verify that a hyperlink range overlapping an unrelated `MergedRegion` is accepted (confirms merge/hyperlink overlap is intentionally not cross-validated)
- Verify that validating a single large hyperlink range completes without cost proportional to its cell count
- Verify that a validation failure registers nothing at all, including ranges earlier in the list that individually passed validation
- Verify that an empty range list is a no-op `Ok(())`
- Verify that a range count at exactly `MAX_HYPERLINKS_PER_SHEET` is accepted and one past it is rejected as `Error::TooManyHyperlinks`, before the O(N²) loop runs (mirrors `resolve::merge`'s `region_count_over_the_limit_is_too_many_merged_ranges`)
- End-to-end (via `pipeline.rs`'s own test suite, not this module in isolation): a sheet whose hyperlink ranges are arranged to maximize simultaneous row activity, plus many unrelated cells, completes JSON generation without cost proportional to cells × ranges (the same class of regression `tests/security.rs`'s `sparse_merge_bounding_box_does_not_amplify_json_generation_cost` guards for merges)

## Open Questions

1. **Interaction with merged regions when a hyperlink `ref` addresses a non-origin (virtual) merged coordinate**: not specially detected. `Sheet::finalize_hyperlinks`'s origin backfill only checks whether the coordinate exists in `cells` at the time it runs (after `resolve::merge::resolve` has already dropped every non-origin merged coordinate via `finalize_merges`) — if a hyperlink's range start happens to land on such a coordinate, backfilling reinserts it as a new, independent blank cell rather than folding it into the merge's origin, and it would then appear in `iter_cells`/JSON output as a distinct cell alongside the merge. Believed not to occur in real-world files (Excel's UI has no path to address a merge's virtual cells independently of its origin), so left unhandled for this first cut rather than solved speculatively.
2. **`resolve::hyperlink::resolve`'s ordering relative to `resolve::resolve_sheet`**: currently called from `pipeline.rs` strictly after `resolve::resolve_sheet` (and thus after merges are finalized), since hyperlink range resolution needs the ZIP I/O step (`r:id` → Target) that can only start once Phase 3's streaming parse has produced `pending_hyperlinks`. This ordering is what makes Open Question 1 possible in the first place; reordering to run hyperlink resolution before merge finalization was considered but not pursued, since it would just move the same edge case to the opposite interaction (a merge whose origin coincides with an already-hyperlinked cell) without eliminating it.
3. **Whether `MAX_HYPERLINKS_PER_SHEET` should eventually be independently tuned rather than borrowed from `MAX_MERGE_REGIONS`**: real-world sheets are expected to have far fewer hyperlinks than merges (hyperlinks are typically added one cell/range at a time via manual editing, not generated in bulk by templates the way merges sometimes are), so 20,000 is likely a conservative cap in practice. Revisit if a legitimate file is ever seen to need more.
