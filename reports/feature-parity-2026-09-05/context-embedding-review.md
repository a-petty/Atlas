# Context and embedding integration evidence

2026-09-05. Worker `context_eval`; source changes staged under `work/context-overlay` for parent integration.

## Context

- Rejected-candidate packing initially re-tokenized/re-serialized the entire accepted prefix for each candidate. On 10,000 small in-memory source files, this took 16.79 seconds. Cached prefix accounting reduced the same assembly to 0.779 seconds cold / 0.761 seconds warm, with 229 whole chunks delivered and 9,771 budget omissions reported. These are packer-only measurements with controlled retrieval and the prior Rust skeleton binary, not an end-to-end product benchmark.
- Existing context tests: 26 passing.
- Added context delivery tests: six passing against the prior Rust binary; the real three-file-chain test reproduces the expected typed-edge integration failure until the corrected extension is built. Context packer source is independently exercised by the passing tests.
- Eight benchmark accounting tests pass: no unsupported skeleton-label credit, exact byte coverage, duplicate-span union, manifest/delivered-source agreement, prior-result parsing, stratified frozen sample and failure denominators.
- Frozen 50 ContextBench tasks: 10 per Python, JavaScript, TypeScript, Rust, Go; ten prior tasks excluded. Dataset revision `c2855792b006af41c67202d33883fb9d46362853`, parquet SHA256, full repository commits and seed are in the manifest. Comparative evaluation has not yet run.

## Embeddings

- Thirty existing embedding tests plus eight new lifecycle tests pass.
- New tests cover edit invalidation, restart reuse, renamed module prefixes, no live-disk fallback for missing accepted source, partial-preparation failure, <=64 model batches, long-query tails, atomic replacement failure, corrupted cache recovery and invalid engine output.
- Actual local BGE-small model loaded offline in 92 ms. Its untruncated tokenizer split a 14,010-character Unicode/punctuation input into 21 chunks and a 16,908-character declaration into 27 chunks. Every chunk was <=480 actual model tokens, and the input tail remained present.
- Actual semantic-search smoke: the embedding-cache function ranked first over a butterfly-plotting function. Cold query/file preparation was 12.98 ms; warm query was 2.60 ms. The cached model is in TMPDIR's `fastembed_cache` directory; no model download was used.
- A prepopulated 10,000-file, one-vector-per-file, 384-dimensional matrix took 29.84–43.83 ms for ten warm ranked searches, p95 43.18 ms. Matrix memory was 15,360,000 bytes. Preparing a reduced ten-file set pruned the live vector cache, content-key cache and matrix to ten files. This isolates matrix/ranking overhead with a deterministic fake model; it is not a full 10,000-file repository/model benchmark.

## Outstanding integration

- Parent must copy any newer overlay files before fresh-source Rust/Python/MCP checks.
- Full 50-task retrieval run and whole-product 10,000-file measurements remain parent-owned.
- Persistent disk cache needs an eviction budget to satisfy bounded cache growth; proposed default is configurable 2 GiB.

## Final worker verification after fresh-extension integration

- **85 tests pass**: all original context/embedding tests plus new context-delivery, retrieval-accounting and embedding-lifecycle suites. The real typed-edge three-file-chain regression now passes against the freshly rebuilt extension.
- Independent skeleton-worker review caught two context defects in the initial implementation: an explicit file's skeleton could lose budget to lower-priority full files, and Unicode line separators inflated full-source line numbers. Both are fixed with regressions. Context admission is now one ranked pass, with immediate full-to-skeleton fallback.
- JSON token accounting initially excluded manifest metadata: an 8,000-token setting could emit 21,307 JSON tokens. Final packing checks both text and default JSON token counts, and both character counts. A 10,000-file packer fixture after this stronger constraint took 1.52–1.57 seconds (64 delivered chunks, 6,942 JSON tokens, 1,399 text tokens); a subsequent safe early-budget optimization was added and covered by the final suite.
- Persistent embedding storage is capped at **2 GiB by default**, configurable through `cache_max_bytes` or `ATLAS_EMBEDDING_CACHE_BYTES`. It evicts oldest-accessed entries on startup, after each 64 writes, or immediately when this worker exceeds the budget. Oversized entries are retained only in memory. Dead-worker temporary files are cleaned up.
- Persistent keys now include streaming SHA256 of actual model weights, configuration and tokenizer files. Model/config mutations receive different keys. The hash does not load a second full copy of model weights into RAM.
- Matrix construction now makes one stack copy and uses rowwise einsum for normalization, avoiding per-file array copies and a matrix-sized square temporary.
- `retrieval-fixture-smoke.json` records an actual Rust + local BGE pipeline smoke: all four retrieval strategies at both 8k and 12k budgets, eight rows, zero errors. The tiny local fixture gives every strategy full gold-byte coverage; this is execution evidence only, not ContextBench quality evidence.
- Parent review item: session/MCP coverage metadata needs exact character and token reservation, since a fixed 2,048-character reserve can be exceeded by twelve long/Unicode invalid-file paths and messages. No worker edits were made to session/MCP files.

## Measured 10,000-file runtime context repair

The release fixture initially missed its context gate at p95 6.587 seconds. A worker-side cProfile on the actual `RepositoryState.execute` path isolated 50,337 `Path.resolve` calls (503,370 `lstat`s, 2.1 seconds) and 147,693 tokenizer calls (3.0 seconds), mostly for source chunks that could not fit.

The context manager now treats graph-returned paths as accepted snapshot identities, resolving only new external aliases. It measures the minimum mandatory file header plus manifest descriptor before reading/parsing a candidate. When even that metadata cannot fit, it reports a file-level budget omission immediately. The first descriptor-only check was insufficient: diagnosis showed 125 remaining tokens while a rejected declaration needed 112 descriptor tokens plus 45 rendered header/content tokens. Including the mandatory header resolved this without a fixed candidate cap.

On the exact five-language, 10,000-file fixture with real cached BGE vectors and production worker/session behavior:

- Three unprofiled warm contexts took **1.235, 1.225, and 1.227 seconds**.
- All returned **38 files / 45 whole source chunks**, matching the previous selected-file and chunk counts.
- Tokenizer calls fell from **147,693 to 40,393**; `fits` calls fell from **31,891 to 47**.
- Budget omissions are now counted at file level when an entire unread candidate cannot fit, rather than parsing it merely to count omitted declarations.
- **59 context and benchmark tests pass**, including source-priority, Unicode line/span/budget tests, a regression forbidding live-path resolution for accepted identities, preservation of explicit symlink alias resolution, and a bounded source-read regression.

Profiles: `scale-context-optimized-1.prof/.txt` and `scale-context-optimized-2.prof/.txt`. Reproducer: `work/profile_scale_context_optimized.py`. These measurements are a focused trial; parent still owns the official ten-sample scale-gate rerun and updated aggregate result.

## Final retrieval-runner audit before the frozen quality run

The parent aligned benchmark eligibility with production `RepositoryState`, including captured buffers, resolver configuration, ignored files, and parse-invalid coverage. Audit verified physical LF gold ranges, including CRLF and Unicode separators. Runner-only corrections now set immutable captured embeddings to generation-managed mode, count the actual text coverage footer, and include actual MCP rendering in warm delivery timing. Assembly timing remains separately reported; delivery timing explicitly covers both MCP response formats. Both rendered token budgets are checked, and transport rendering enforces the character ceilings.

Nineteen benchmark regressions pass against the fresh extension (0.78 seconds). The integration test uses real capture, Rust parsing, context packing, and MCP renderers with deterministic embeddings. It verifies identical eligible file hashes for all eight strategy/budget rows; excluded ignored/generated/parse-invalid sources; retained invalid-source diagnostics; actual rendered token totals and budget limits; and rendering within the timed delivery scope. Resume/source/model identity tests remain green.

The final actual BGE eight-row smoke is queued after the independent scale timing window. No production ranking changes were made during this audit. Parent must copy only `benchmarks/bench_retrieval.py` and `python_shell/tests/test_retrieval_benchmark.py` from `context-overlay` for this increment.

## Required-span budget floor and captured-prefix correction

The formal synthetic/public runs exposed a narrow remaining-budget interval: an empty file descriptor/header fit, while every actual declaration's mandatory retained span did not. The previous early rejection then parsed/tokenized all remaining files. The current structured skeleton emitter guarantees each nonempty chunk retains original bytes, so the lower bound now includes one smallest valid source span. Older string-only skeleton compatibility keeps its previous empty-span floor.

Nine calibrated regressions vary three queries and three budgets. Before the fix they read all 100 fixture files; afterward they read four or five and deliver the same four or five chunks. On the same captured 10,000-file graph, the failing batch query changed from 3.198 seconds to 1.409 seconds; three optimized queries took 1.375–1.409 seconds. Exact admitted file/mode/spans/text were identical before and after. On the same Django graph, model-field and form-field query tails changed from 6.128/7.365 seconds to 0.728/0.777 seconds, again with identical admitted source. These are controlled trials, not the official ten-sample gate reruns.

An actual graph regression also confirmed that replacing accepted `alpha.py` with a symlink to `beta.py` changed the old embedding module prefix to `beta` while its source remained captured `alpha`. Authoritative captured graphs now derive prefixes from lexical accepted IDs; standalone callers retain live symlink resolution. This preserves generation coherence.

All 113 context, embedding, and benchmark tests passed against the fresh extension after these changes. Django edit cProfile after the span fix took 2.370 seconds: reconciliation 1.698 (Rust update 0.863 plus scans 0.829), context 0.671. Unprofiled edit samples and the official scale reruns remain pending. Evidence files: `context-span-rejection-trial.json`, `public-context-tail-trial.json`, and `public-edit-profile-1.txt/.prof`.

## Bounded metadata reuse after the remaining edit diagnosis

The first unprofiled Django edits after the span floor were still borderline: 2.001, 1.951, and 2.043 seconds. The profile showed approximately 11,196 duplicate descriptor/header tokenizations among 11,864 calls. A per-context-manager LRU now caches only those pure path/reason/structured-emitter costs. It is capped at 16,384 entries, excludes keys longer than 2,048 characters, and never caches source contents or query budgets.

All 115 focused tests pass, including warm/cold result equality, changed captured source still appearing after a cache hit, LRU eviction and oversized-key rejection, and old-emitter mode separation. Real Django cold/warm delivery results were exactly equal. Its 2,799 cached entries used an upper estimate of 1,471,209 bytes (shared strings counted repeatedly). Three unprofiled edit-through-context trials after warming took **1.661, 1.674, and 1.679 seconds**, compared with the earlier borderline samples. Official reruns remain parent-owned. Evidence: `public-edit-metadata-cache-trial.json/.log`.

## Final release handoff and quality-run cache provenance

The parent rebuilt the release extension and passed the complete verifier (190 Python tests plus Rust). The final actual BGE/Rust/RepositoryState/MCP-render fixture smoke passed all eight strategy/budget rows, zero errors, exact gold source spans, common eligibility, and both rendered ceilings (`retrieval-delivery-smoke.json/.log`).

The independent official reruns passed every measured gate: synthetic context p95 1.095 seconds/edit 0.885; public Django context 0.678/edit 1.723. Public prepared-cache peak RSS stayed at 1.123 GB through ten edits. Production and benchmark code are now frozen for the quality run.

After all timing processes finished, 2,799 public-scale embedding files were copied into the retrieval cache without overwriting its 20 existing entries. The run starts with 2,819 entries totaling 38,641,135 bytes. `cache-provenance.json` records every copied key/hash and explicitly labels warm persistent-cache reuse; preparation durations must not be presented as fully cold-cache timings. The frozen run is writing `retrieval-results-final.json` with its log in `retrieval-final.log`.

`work/analyze_retrieval_results.py` is prepared for the completed 400-row result. It verifies completeness/equal eligibility/budgets, preserves failure denominators, reports known gold bytes versus unmeasurable blocks, language/budget metrics, paired Atlas comparisons and repository-cluster bootstrap uncertainty. Final interpretation must address incremental MCP capability and useful tokens, distinguish retrieval from coding outcomes, and report unfavorable comparisons without tuning the frozen sample.

## Analysis preparation while the frozen run remains active

The analysis helper now reports actual text/JSON token deltas on explicitly counted successful task pairs, illustrative best/worst paired outcomes, and unique-task graph/embedding/lexical preparation distributions and totals. Embedding setup is not described as a BM25 requirement; it is a shared experimental preparation stage, and its timing reflects mixed persistent-cache reuse.

Gold-scope annotations supplement the unchanged primary denominators. Delivery by any arm proves a file was accepted; unsupported extensions, fixed ignored directories, and bounded invalid-source diagnostics establish known exclusions. Undelivered supported files with no explicit exclusion retain unknown eligibility. The first Ansible task includes one YAML gold file among six, establishing a known file-recall ceiling of 5/6. Exact excluded-byte annotations will be computed after completion from pinned gold ranges and checkouts, with their sums checked against the recorded known-byte denominator.

Paired uncertainty uses observed repository clusters, retaining all within-repository task pairs. These are descriptive sensitivity intervals, not confirmatory tests: bootstrap resamples do not preserve fixed language quotas and there is no adjustment across contrasts. A single independent repository does not receive a spurious zero-width interval. The recorded fingerprint's internal digest validates and includes all required runtime/runner/extension labels among 49 files; a current-source/extension rehash remains deferred until the final run completes. No production or benchmark code changed during this analysis preparation.

The post-run analysis will also verify eligible IDs without indexing: run the unchanged source-manifest scanner, subtract complete recorded invalid-file diagnostics, and classify previously undelivered gold files only when both hash and count exactly match the recorded accepted set. Dirty/wrong-HEAD checkouts, truncated diagnostics, canonical-path aliases, and hash mismatches remain explicit unknowns. This reconstruction is secondary evidence and does not change any metric denominator. Response-token interpretation will name the actual Atlas/tiktoken encoding and avoid claims about total coding-session tokens or billing.

Analysis preparation follow-up: actual analysis/fingerprint verification must use `work/locked-venv/bin/python`, matching the frozen runner and verifier (the repo `.venv` is older). A mismatched current/saved fingerprint now suppresses eligibility policy replay and reports classifications as unknown. Lightweight stdlib checks passed for failed-row quality denominators, successful-response-only token means, unique-task setup deduplication, missing timing handling, and repository-cluster bootstrap behavior when fewer than two repositories are available. Full 400-row analysis remains pending completion of the parent-owned frozen run; no production/benchmark code or active-run cache was changed.
