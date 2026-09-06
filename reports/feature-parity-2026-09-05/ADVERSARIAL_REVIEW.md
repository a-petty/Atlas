# Final independent consistency review — 2026-09-05

Scope: Rust graph incremental updates/removals and typed semantic edges, bounded watcher delivery, and Python session generation ownership. This is a focused follow-up to the previous session lifecycle review, not a repetition of its five already-fixed findings. No production files were changed by this review. Reproductions use the actual Atlas extension or link directly to production watcher.rs; they make no network or model calls.

The accepted plan requires incremental/fresh graph equivalence, removal of semantic evidence when its binding disappears, request-time reconciliation of missed watcher events, accepted source buffers, and generation-bound pagination. The standalone watcher API is also reviewed for bounded-queue recovery; loss of a hint alone does not imply a stale RepositoryState result.

## Confirmed findings and disposition

1. **P1: Local-tier edits skipped semantic-edge reconciliation.** Adding a parameter named `foo` to `def run(): foo()` after `from b import foo` retained a SymbolUsage edge in the incremental graph; a fresh graph retained only Import. Call answers already correctly became empty. The update reported zero removed edges and no PageRank recalculation. A second reproduction removes `export` from an otherwise identical JS function: callers in another file retained obsolete SymbolUsage edges although call resolution correctly became empty. Identifier/import-path hashes do not capture these binding/export changes. Parent implemented reconciliation for both old and new affected call-index files in the Local branch. Independent verification of the rebuilt extension is pending.

   Reproductions: `probe_graph_local.py`, `probe_js_export.py`. Both assert incremental dependencies equal a fresh build and failed before the fix. The JS case verifies invalidation in an unchanged importing file.

2. **P2: A symlink retarget could cross source files within one pagination generation.** With both a.py and b.py already indexed and link.py pointing to a.py, the first one-item skeleton page returned alpha_0. Retargeting link.py to b.py yielded bravo_1 with the old cursor and unchanged generation. Native watching was disabled, isolating pre-request reconciliation. The scan canonicalized and deduplicated aliases, while path resolution consulted live symlink targets. Parent implemented captured alias topology in the generation manifest and generation-owned path translation. Independent verification of the rebuilt extension is pending.

   Reproduction: `probe_session_symlink.py`. Desired post-fix verification is in `verify_fixes.py`: path translation remains at the captured target until reconciliation, retargeting advances the generation, the old cursor is rejected, and a new query starts at bravo_0.

3. **P2 for standalone FileWatcher: output overflow permanently discarded persistent changes.** Creating 6,000 .py files while leaving the output queue undrained yielded 4,096 notifications and no subsequent recovery; 1,904 paths were never delivered. Both reconciliation and native sends ignored Full, while reconciliation advanced its baseline as if delivery succeeded. This was independently reproduced twice. RepositoryState's separate pre-request manifest reconciliation compensates, so the reproduction does not establish stale MCP/session answers. Parent now advances the delivery baseline only for successful sends and retries full-queue changes. **Independently verified fixed:** first drain 4,096, subsequent recovery 1,904, unique observed 6,000, missing zero; 11 overflow retries reported, stop completed in 19 ms.

   Reproduction crate: `../watcher-audit`; test `output_overflow_must_reconcile_after_consumer_resumes`. It includes production watcher.rs by path. The initial run failed; the post-fix run passed in 6.81 seconds. Separate output-pressure shutdown testing also passed before the fix; no shutdown deadlock is claimed.

## Limits

The watcher channels bound message counts, while native message batches can contain multiple events. This review did not measure an RSS ceiling under sustained native event production, nor establish a shutdown deadline for slow filesystem scans. Those are measurement limits, not independently reproduced defects. Existing locked verification and performance results are owned by the parent implementation task.

## Run the focused verification

From `/Users/apetty/Dev/Atlas`, after rebuilding the real extension:

```sh
.venv/bin/python /Users/apetty/Documents/Codex/2026-09-05/i/work/final-audit/verify_fixes.py
```

For watcher overflow, from `work/watcher-audit`:

```sh
CARGO_HOME=/Users/apetty/Documents/Codex/2026-09-05/i/work/cargo-home cargo test --offline output_overflow_must_reconcile_after_consumer_resumes -- --nocapture
```
