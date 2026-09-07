# Atlas implementation state

Completed September 5, 2026. Branch: `codex/atlas-feature-parity`.

All agreed implementation and evaluation stages are complete. No benchmark workers remain running. Read `IMPLEMENTATION_REPORT.md` for the evaluation and `reports/feature-parity-2026-09-05/REPRODUCE.md` for exact evidence/reproduction scope.

## Decision supported by the evidence

Atlas is useful for targeted structural navigation and compact API views. The frozen retrieval comparison does not establish an advantage for graph expansion over flat embeddings or expansion-disabled Atlas. Aggregate advantages over BM25 are small and uncertain; the ten Python tasks strongly favor BM25. Keep automatic assembly optional. Whole-agent token savings and coding-task accuracy remain unmeasured, as the agreed scope excluded a paid coding-model trial.

## Completed and verified

- Graph/call correctness, five-language API skeletons, captured source generations, shared MCP/CLI session implementation, cancellation/restart behavior, persistent embeddings and complete response budgets are implemented.
- Independent review findings were reproduced and closed with regressions, including stale semantic edges, symlink generation changes and dropped watcher events. Detailed reports and passing closure evidence are bundled.
- The production verifier passed 313 Rust tests, rebuilt the release extension and passed 190 Python tests. After the evaluator-only correction, all 209 Python tests passed in 7.28 seconds against that same extension. No Rust tests were ignored. Remote CI has not run.
- All agreed warm gates pass on both the 10,000-file fixture and pinned Django. Context p95 is 1.095/0.678 seconds; edit-to-context p95 is 0.885/1.723 seconds respectively. These are finite fixture measurements.
- The corrected frozen comparison completed all 50 tasks / 400 measurements with zero errors or token/character violations. The current implementation fingerprint, dataset/pinned checkouts, model identity and gold-byte replay checks pass.

Production commit: `d90e59d90701bc9059385e80f9c626dca77f846a`. Evaluator correction: `0580d7b674cddfdbae21c63b67b518925b601beb`. Release extension SHA256: `80a276b6d0bbfba972a408a702515916236ccbdd9c56672f0a61788a511ba3a5`. Corrected implementation fingerprint: `0e6413797e2fff31ae4bb8f164c841953b7a4ead3f8904b621ff9041e50b9e93`.

## Evidence and limitations

The evaluator I added initially failed on absolute gold paths in 17 tasks. The full schema-2 run remains preserved with 400 rows / 136 error rows. The independently reviewed schema-3 correction canonicalizes verified in-repo container paths and aliases, preserves original provenance, and keeps external/invalid annotations explicit without reading outside a checkout. Ranking, packing, model and the complete frozen sample were unchanged. The corrected run finished at 2026-09-06 02:53:07 UTC.

All 264 originally successful rows match on delivered-file lists, text/JSON token counts, accepted-source hashes/counts and engine keys. Four 8k rows differ by two characters in both text/JSON counts; their gold and quality metrics match. Exact old response text was not saved, so this discrepancy remains unresolved and byte-identical responses are not claimed. Accepted-ID replay is verified for 39 tasks and unknown for 11 because invalid-source diagnostics were truncated. The report retains those limits and empty/unmeasurable gold in the primary accounting.

The first post-grader test invocation omitted a writable task cache and had four permission failures. A task-local cache resolved the environment error; both logs are preserved. The passing rerun required no source change.

Cold semantic preparation is expensive: observed Ansible/MUI snapshots required roughly 19/21 minutes. The original recorded setup is an incomplete lower bound of 2.47 hours across 33 tasks. The corrected run reused 104,769 prepared entries and needed 334.1 combined setup seconds across all 50. Graph/symbol/skeleton/call tools do not require embeddings. Configure server/client deadlines for large cold semantic queries; the default 120 seconds is insufficient for the observed large preparations.

The evidence bundle is mirrored in repo `reports/feature-parity-2026-09-05/` and task `/Users/apetty/Documents/Codex/2026-09-05/i/outputs/atlas-implementation/`. It includes corrected and original raw results, paired/per-language analysis, scope/provenance checks, original failures, scale evidence, verifier logs, reviews, catalog accounting and a selected source/skeleton illustration. `EVIDENCE.sha256` records bundle hashes. The packager is task `work/package_atlas_evidence.py`.

## Recovery and reproduction

Original HEAD: `8a1962e5cb7fd7603a544b9f0687a130d852ab40`. April dirty resolver work and the JS CPG test are preserved in `357ae6b`; accepted plan in `7680296`. Implementation commits are `0892fb3`, `2ed3384`, `a796d49` and `d90e59d`; the evaluator correction is `0580d7b`. Final documentation/evidence are in the subsequent local commit. Original untracked editor files and historical documents remain untouched. At that implementation-completion checkpoint, nothing had been pushed or deployed.

The verified Python is `/Users/apetty/Documents/Codex/2026-09-05/i/work/locked-venv/bin/python` (3.11.14). The repository's older `.venv` was not used for this evidence. Build caches are task-local. The pinned public checkouts, model cache and temporary work files are not bundled with the report. The original comparison is task `work/implementation/retrieval-results-final.json`; its bundled name is `retrieval-results-before-path-correction.json`. The final comparison is `retrieval-results-corrected.json`.

For a future fresh-source verification, follow `REPRODUCE.md`: sync the lock with Python 3.11 and the dev/MCP/benchmark extras, then run `scripts/verify.py --release` with a writable Atlas cache. It rebuilds the extension before Python/MCP tests. Use a new held-out sample if changing the retrieval policy; this cohort has now been inspected.

Remaining capability limits are documented scope: conservative static calls; no Rust/Go import or call resolver; no exhaustive CFG/data-flow audit; descriptive skeletons that need body reads for debugging; and an unmeasured end-to-end agent benefit. These are not unfinished implementation stages.

## September 6 discussion follow-up

`RETRIEVAL_DIRECTION.md` records the subsequent BM25 discussion and the provisional focus on agents revisiting Alec's own repositories. The proposed next experiments remain unimplemented; the completed evidence does not establish whole-agent token savings. Alec requested committing and pushing the current work to GitHub on September 6. Publication targets the existing `codex/atlas-feature-parity` branch on `origin`; no merge or deployment was requested.
