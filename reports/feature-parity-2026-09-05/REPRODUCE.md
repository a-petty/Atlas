# Reproducing the Atlas implementation evidence

The tested production tree is commit `d90e59d90701bc9059385e80f9c626dca77f846a` on the local branch `codex/atlas-feature-parity`. The benchmark's subsequent schema-3 correction normalizes verified repository-container gold paths; it does not change production ranking or packing. `corrected-run-provenance.json` identifies that evaluator commit and the prepared cache used for the complete rerun. The final evidence commit adds documentation, results and standalone analysis. Use a separate checkout for reproduction if the original working tree contains later work.

## Fresh build and functional verification

From `/Users/apetty/Dev/Atlas`:

```sh
uv sync --locked --python 3.11 --extra dev --extra mcp --extra benchmark
uv run --no-sync python scripts/verify.py --release
```

The observed run used Python 3.11.14, uv 0.11.26 and Rust 1.93.0 on macOS 15.7.2 arm64. It ran 313 Rust tests, rebuilt the release extension and passed 190 Python tests, including real stdio MCP tests. `final-verification.log` records that complete output. After the evaluator-only correction at `0580d7b674cddfdbae21c63b67b518925b601beb`, the full Python suite passed 209 tests in 7.28 seconds against the same release extension; see `post-grader-python.log`. CI definitions have not been executed remotely.

For this sandbox, use writable task-local caches:

```sh
CARGO_HOME=/Users/apetty/Documents/Codex/2026-09-05/i/work/cargo-home \
UV_CACHE_DIR=/Users/apetty/Documents/Codex/2026-09-05/i/work/uv-cache \
ATLAS_CACHE_DIR=/Users/apetty/Documents/Codex/2026-09-05/i/work/verification-cache \
HF_HUB_OFFLINE=1 PYTHONDONTWRITEBYTECODE=1 \
/Users/apetty/Documents/Codex/2026-09-05/i/work/locked-venv/bin/python \
  scripts/verify.py --release
```

The final extension SHA256 is `80a276b6d0bbfba972a408a702515916236ccbdd9c56672f0a61788a511ba3a5`. A rebuild on another platform may produce a different binary hash; do not combine its measurements into this run through `--resume`.

The first post-grader Python invocation omitted the writable cache setting: 205 tests passed and four failed on a denied write to `/Users/apetty/.cache/atlas`. That log is retained as `post-grader-python-default-cache.log`. The successful rerun used `ATLAS_CACHE_DIR=/Users/apetty/Documents/Codex/2026-09-05/i/work/post-grader-test-cache` with `python -m pytest python_shell/tests -q -p no:cacheprovider`; no source change was needed.

## Frozen retrieval sample

The manifest is `contextbench_50_v1.json`, mirrored from `benchmarks/manifests/`. Dataset revision: `c2855792b006af41c67202d33883fb9d46362853`; parquet SHA256: `2f56535bdc73eb8a68bf4ebb49789d8e9cd4f219ea60df6290b85278aee61ca8`. The dataset was already cached locally. Fifty pinned checkouts are recorded in `benchmark-checkouts.json` and are not bundled with this evidence.

From the same repository and locked environment, the actual final command was:

```sh
GIT_TERMINAL_PROMPT=0 \
ATLAS_CACHE_DIR=/Users/apetty/Documents/Codex/2026-09-05/i/work/retrieval-cache \
HF_HUB_OFFLINE=1 PYTHONDONTWRITEBYTECODE=1 \
/Users/apetty/Documents/Codex/2026-09-05/i/work/locked-venv/bin/python \
  -m benchmarks.bench_retrieval run \
  --dataset /Users/apetty/.cache/huggingface/hub/datasets--Contextbench--ContextBench/blobs/2f56535bdc73eb8a68bf4ebb49789d8e9cd4f219ea60df6290b85278aee61ca8 \
  --repo-cache /Users/apetty/Documents/Codex/2026-09-05/i/work/contextbench-repos \
  --output /Users/apetty/Documents/Codex/2026-09-05/i/work/implementation/retrieval-results-corrected.json
```

Choose a new output filename for a new run. An interrupted run can use `--resume` only when its manifest, dataset, dependencies, source, loaded extension, model and run configuration match. Failed or partial tasks rerun as a unit; successful complete tasks can be reused. The final evidence includes failures rather than dropping them.

No source in the public repositories was executed. All inference used local FastEmbed BGE small English v1.5; no paid coding agent was invoked. `HF_HUB_OFFLINE=1` requires already cached model files. `cache-provenance.json` records the original persistent cache reuse from the pinned Django scale preparation plus earlier locked-environment entries. The corrected comparison reuses embeddings prepared during the complete original run; `corrected-run-provenance.json` records its starting cache state. Preparation times are therefore reported separately for the original and corrected runs and are not entirely cold startup measurements. The original error record remains in `retrieval-results-before-path-correction.json`.

All four arms use the same source representation and packer. BM25 ranks the same path/skeleton embedding text. Actual text and JSON responses both count against the 8k/12k budgets and 60,000-character ceiling. Token accounting uses `cl100k_base`, not the host model's entire conversation. Absolute paths and metadata consume space; changing the checkout root can change packing and must be recorded when comparing runs.

For the complete analysis, including current-source verification and annotated-source eligibility, run from `/Users/apetty/Dev/Atlas`:

```sh
PYTHONDONTWRITEBYTECODE=1 \
/Users/apetty/Documents/Codex/2026-09-05/i/work/locked-venv/bin/python \
  reports/feature-parity-2026-09-05/analyze_retrieval_results.py \
  reports/feature-parity-2026-09-05/retrieval-results-corrected.json \
  --atlas-repo /Users/apetty/Dev/Atlas \
  --original-result reports/feature-parity-2026-09-05/retrieval-results-before-path-correction.json \
  --dataset /Users/apetty/.cache/huggingface/hub/datasets--Contextbench--ContextBench/blobs/2f56535bdc73eb8a68bf4ebb49789d8e9cd4f219ea60df6290b85278aee61ca8 \
  --repo-cache /Users/apetty/Documents/Codex/2026-09-05/i/work/contextbench-repos
```

The analysis writes a sibling `retrieval-results-corrected.analysis.json`. Use the same locked environment; the repository's older `.venv` was not the environment used for the final run. The source, dataset and checkout arguments support full provenance and eligibility annotations; `--original-result` also compares recorded delivery metadata against successful measurements before the grader correction. These checks run without model preparation or graph indexing. Running only `python analyze_retrieval_results.py retrieval-results-corrected.json` produces the basic statistical analysis without that complete scope supplement. It requires all 400 expected task/method/budget rows, checks duplicate keys, delivery limits and equal eligibility, and retains failed measurements as zero in quality means. It reports paired per-task comparisons and repository-cluster bootstrap intervals. Invalid gold ranges are explicitly identified rather than silently treated as measurable bytes.

## Tool catalog accounting

The live local MCP catalog can be measured without starting a repository session or embedding model:

```sh
HF_HUB_OFFLINE=1 PYTHONDONTWRITEBYTECODE=1 \
/Users/apetty/Documents/Codex/2026-09-05/i/work/locked-venv/bin/python \
  reports/feature-parity-2026-09-05/measure_tool_catalog.py \
  --atlas-repo /Users/apetty/Dev/Atlas \
  --output /path/to/new-tool-catalog-accounting.json
```

The included result stores the descriptors, instructions, source hash, serialization hash and `cl100k_base` count. It measures compact JSON rather than the client's model prompt, tool-loading policy, caching or billed usage.

## Scale

`scale-validation-report.md` includes the exact commands, all targets, fixture identities and measurement scope. The final `scale-accepted.json` and `public-scale-accepted.json` use prepared persistent caches. Their ten-sample p95 is the maximum observed sample, not a universal latency guarantee.

Synthetic fixture: exactly 10,000 files, 2,000 per language, seed 20260905, 3,338,612 source bytes. Fixture manifest SHA256: `cd091ac6833e0fc977529774fe5b47a41867edffbda349a5286a52faf37ede14`.

Public fixture: Django `8c3bd0b708b488a1f6e8bd8cc6b96569904605be`, isolated editable copy at `work/public-scale-django`. `public-scale-copy-provenance.json` and the accepted manifest identify its bytes. Edit trials restored original content.

The earlier public cold timeout, resumed run and failing performance results remain in the package. Do not replace those files with later passing runs or interpret prepared-cache initialization as cold inference. Run reproduction without simultaneous compilation or other model-heavy jobs; competing workloads invalidate latency comparability.

## Reading the evidence

Use the implementation report for conclusions. Raw retrieval JSON is the measurement record; its analysis JSON contains aggregations. The scale report explains operating cost. The independent benefit-claims review describes what the experiment can and cannot establish. Passing engine tests and better annotated-source recall do not establish total agent-token savings or coding-task success.
