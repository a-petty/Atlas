# Atlas: implementation and honest value assessment

September 5, 2026. Tested production commit: `d90e59d90701bc9059385e80f9c626dca77f846a`; corrected evaluator: `0580d7b674cddfdbae21c63b67b518925b601beb`, on `codex/atlas-feature-parity`. No paid coding-model trial, push or deployment was performed.

## Judgment

Atlas adds useful capability to a model's working environment: it can answer structural questions and present an API surface without requiring the model to reconstruct both from repeated searches and full-file reads. Its strongest current case is targeted repository navigation, particularly in unfamiliar Python and JavaScript/TypeScript code. I would use those capabilities in real work, with ordinary source inspection and tests available alongside them.

The frozen comparison does not justify making automatic context assembly the default. Full Atlas shows no demonstrated retrieval advantage over flat embeddings or Atlas without dependency expansion. Its modest aggregate advantages over BM25 have wide repository-cluster intervals that cross zero, while the ten Python tasks favor BM25 substantially. It chooses what a model gets to see, so a relevance error or an omitted implementation body can outweigh a large compression ratio.

The implementation work improved reliability and made this selection policy measurable. The resulting measurements do not show that graph expansion earns its place in automatic retrieval. That finding weakens the automatic-assembly claim while leaving a useful case for asking the graph explicit structural questions.

I cannot honestly claim a measured reduction in total agent tokens or an increase in completed coding accuracy from this work. We measured source delivery and system behavior. The experiment does not include a model solving tasks, choosing follow-up reads, editing code or running the task's correctness tests.

## What is incremental

| Atlas feature | What a model gains | Where the benefit weakens |
|---|---|---|
| Callers, callees and typed dependencies | A targeted structural answer can replace searches, import tracing and disambiguation. Supported lexical bindings reduce false matches between unrelated names. | Equivalent language-server tools overlap with this capability. Dynamic behavior remains unresolved; Rust/Go dependency and call analysis are unavailable. |
| API skeletons and declared symbols | A compact view of signatures, types, imports and documentation can help locate extension points before reading bodies. Retained source spans make the representation inspectable. | Bug fixes often depend on a condition, mutation, initializer or error path that a skeleton omits. Another source read may be necessary. |
| Semantic file ranking | Natural-language queries can surface candidate files when the task does not name the right identifier. | Similar vocabulary is not necessarily task relevance. A model using adaptive lexical searches is a stronger baseline than a single fixed query. |
| Automatic context assembly | A bounded package of source and supporting structure reduces orchestration work. Explicit files retain priority. | The policy can include irrelevant high-centrality neighbors, omit a needed body, or spend space on metadata. More retrieved context need not mean more useful context. |
| Fresh, shared repository generations | Related tools answer from captured source, and ordinary edits are reconciled automatically. This reduces stale or contradictory evidence during a session. | Consistency is not completeness. The model still needs to recognize coverage limits and changes between generations. |

These are gains in access and computation. Atlas derives its answers from the same repository that ordinary file tools can read; it does not add otherwise unavailable knowledge. Moving repeated structural work into a local index is nevertheless valuable when the alternative is spending model attention and context on reconstructing it.

MCP makes that analysis available as explicit model actions. The underlying indexes and source representations supply the capability; the CLI exposes the same engine. The transport alone establishes no token or accuracy gain.

The maintenance cost also matters. Atlas now owns a static resolver, incremental invalidation, source capture, worker lifecycle, several language representations and a retrieval policy. The fresh-build verifier and adversarial regressions make that more defensible, but these components will need continued attention as language and client behavior change. I would require evidence of workflow benefit before adding more analysis layers.

The strongest use case is repeated investigation of an unfamiliar, substantial repository. For a known file and a narrow change, a direct read can be the most efficient next action. For a client already exposing good symbol and language-server navigation, Atlas must earn its place through better representation, cross-tool consistency or measured retrieval quality.

## Token savings and accuracy are separate claims

A skeleton can contain substantially less text than a full file. Net agent-token savings require a larger accounting:

> Avoided source reads, search output and repeated investigation, less Atlas's tool definitions, arguments, metadata, pagination, repeated context and follow-up body reads.

The current comparison counts delivered text and JSON with `cl100k_base`; it does not count the host's complete tool catalog, conversation history, reasoning, generated code, repeated prompt charging or model-specific tokenization. The tool catalog cost depends on how the client loads tools. The same numerical budget is therefore an estimate for a different tokenizer, not a universal limit on a model's full context.

For a concrete sense of overhead, Atlas's thirteen current MCP tool descriptors plus server instructions occupy 1,756 `cl100k_base` tokens when serialized as compact JSON. That measurement reads the actual local catalog without starting a repository or embedding model. It is not a measurement of the client's prompt format or billed usage: clients can transform, defer or cache tool definitions. It does show why a small source reduction alone is insufficient evidence of net savings.

A particularly wasteful workflow would be search, skeleton, automatic context, then the same full-file read the model needed initially. A useful workflow would use a targeted relationship or skeleton to choose a small number of necessary reads. Atlas exposes the ingredients for the latter; this evaluation does not measure how often a model chooses it.

A deliberately selected example makes the tradeoff concrete. Atlas's own implementation-heavy `session.py` contains 5,569 tokens of source; the direct parser's plain skeleton contains 603, an 89.2% reduction. It omits the second manifest comparison, the cancellation-ownership check and the fallback kill/join logic. Those bodies are necessary evidence for debugging those behaviors. Reading the skeleton and then the entire file would consume 6,172 content tokens before any tool wrappers, compared with 5,569 for reading the file first. A targeted follow-up read could preserve savings. This single-file illustration is separate from the frozen benchmark and excludes MCP page wrappers; it is neither representative compression nor measured agent savings. Exact captured source, parser response, spans and omitted checks are preserved in `skeleton-token-illustration.json`, and the response was independently reproduced.

Accuracy can improve when the model sees the right evidence and avoids a false relationship or stale result. It can decrease when compression hides the decisive branch, retrieval misses the relevant file, or the model treats a static call list as exhaustive. Correctly resolving the cases in the regression suite supports the reliability of those answers. It is not an estimate of a model's overall coding accuracy.

## Frozen retrieval comparison

All 50 tasks completed under the corrected evaluator: 400 measurements, zero execution errors, zero token/character budget violations and matching accepted-file hashes across methods. The table contains macro averages over all 50 tasks, including the three with no measurable annotated bytes. File F1 balances delivered reference files against unrelated delivered files; known-byte recall credits only retained annotated source.

| Method | File F1, 8k | File F1, 12k | Known-byte recall, 8k | Known-byte recall, 12k |
|---|---:|---:|---:|---:|
| BM25 | 12.69% | 13.37% | 3.61% | 8.04% |
| Flat embeddings | 14.06% | 14.16% | 7.29% | 10.01% |
| Atlas without expansion | 13.49% | 13.63% | 7.29% | 9.94% |
| Full Atlas | 13.49% | 13.66% | 7.29% | 9.92% |

Graph expansion changes no quality metric on any task at 8k. At 12k it changes mean file F1 by +0.035 percentage points and known-byte recall by −0.016 points. Flat embeddings match or exceed full Atlas's averages at both budgets. This sample provides no demonstrated retrieval benefit for the additional expansion policy or PageRank anchor promotion.

Against BM25, Atlas's known-byte recall gains are +3.67 points at 8k and +1.88 at 12k. Their 95% repository-cluster bootstrap intervals are [−2.36, +12.19] and [−9.83, +13.16]. File-F1 gains are only +0.80 and +0.29 points, also with intervals crossing zero. These descriptive intervals resample 19 distinct repository URLs; they do not preserve the language quotas and are not adjusted for multiple comparisons. A favorable mean is insufficient evidence of a general advantage.

The language differences matter for Alec's Python-heavy work. On these ten Python tasks, BM25 reaches 33.75% file F1 at both budgets; Atlas reaches 8.64% and 12.64%. Much of Atlas's favorable aggregate byte result comes from Go. These small groups reuse repositories, so this is a warning about the observed sample rather than a universal verdict on Python or Go. The full per-language and paired tables are in `retrieval-analysis-notes.md`.

Actual delivery costs also give little evidence of an Atlas-specific saving:

| Method | Mean text tokens, 8k | Mean JSON tokens, 8k | Mean text tokens, 12k | Mean JSON tokens, 12k |
|---|---:|---:|---:|---:|
| BM25 | 3,632.9 | 6,681.4 | 5,910.6 | 10,688.7 |
| Flat embeddings | 3,457.6 | 6,675.5 | 5,949.1 | 10,683.3 |
| Atlas without expansion | 3,484.4 | 6,676.6 | 5,940.8 | 10,679.1 |
| Full Atlas | 3,485.4 | 6,678.4 | 5,939.5 | 10,678.7 |

Atlas saves 147.5 text tokens against BM25 at 8k and uses 28.9 more at 12k. The baselines share the same compression and packing, so their common reduction cannot be credited to Atlas's ranking. JSON serialization and provenance consume substantial space; because admission enforces both formats, text mode also bears that constraint. Less delivered text with missing relevant source is not demonstrated useful savings.

Coverage limitations are explicit. The cohort contains 522,819 known annotated bytes and 49 unmeasurable gold blocks across 16 tasks. Two tasks have empty gold and one has only external scratch-file gold. Forty-seven have measurable bytes; only 32 have nonempty gold and complete span metrics. On that 32-task companion subset, Atlas's file-F1 difference against BM25 turns negative at both budgets; the broad uncertainty intervals still cross zero. The primary 50-task results are retained separately.

Accepted source IDs were independently reconstructed for 39 tasks. Eleven checks remain unknown because invalid-source diagnostics were truncated; they are not reported as passes. Known exclusions imply optimistic macro recall ceilings of 79.86% for files and 82.23% for known bytes, retaining the primary denominator convention. Unknown exclusions could lower these upper bounds. Source policy does not explain most of the shortfall: at 12k Atlas misses 147 proven accepted gold-file occurrences, while its observed file/byte recall is only 21.05%/9.92%. The saved evidence cannot generally separate ranking misses from packing misses.

Current source/dependency fingerprints, model identities, pinned clean checkouts, normalization mappings and known-byte totals passed replay checks. All 264 originally successful rows have matching delivered-file lists, text/JSON token counts, accepted-source hashes/counts and engine keys. Four 8k rows have text and JSON character counts lower by exactly two; all gold-byte and file metrics for that task match. Exact response text was not saved, so this limited discrepancy remains unresolved and byte-identical responses are not claimed. The original 136 grading-error rows have no comparable saved retrieval measurements.

The evaluator I added initially mishandled absolute annotation paths in 17 tasks; 16 contain repository-container paths and one contains only an external scratch reference. I preserved that complete 400-row run, including its 136 error rows, then applied an independently reviewed schema-3 correction and reran the same complete cohort. The corrected run finished all 400 measurements with zero errors. It normalizes verified repository-specific prefixes, canonicalizes in-repo aliases and preserves external scratch paths as unmeasurable gold. This changes gold-path interpretation, not the frozen selection, ranking or packing algorithms.

The comparison freezes 50 new ContextBench tasks, ten each Python, JavaScript, TypeScript, Rust and Go, excluding the historical ten development examples. It fixes repository commits, seed `20260905`, dataset revision `c2855792b006af41c67202d33883fb9d46362853` and dataset SHA256 `2f56535bdc73eb8a68bf4ebb49789d8e9cd4f219ea60df6290b85278aee61ca8` before running the methods. No ranking tuning followed sample selection. The manifest contains 47 bug-fix tasks and three feature tasks; it does not represent every kind of repository exploration.

Four methods share eligible source, coverage allowance, map/full-source/skeleton packing, 8k/12k budgets and the 60,000-character transport limit. The benchmark validates spans against actual delivered source; a signature does not receive credit for its omitted body. Failures remain in metric denominators. Invalid annotated ranges are reported separately; known-byte recall cannot represent unmeasurable gold blocks.

The annotations identify a reference context rather than an execution oracle. An unannotated dependency may be useful even when it lowers file precision, and byte recall weights a long region more heavily than a short decisive condition. These metrics do not establish whether a model understands the evidence or produces a correct change.

The frozen test evaluates automatic assembly, not a model choosing targeted calls. A loss to BM25 would weaken the case for default automatic assembly without disproving callers or skeleton utility. The BM25 comparison isolates ranking while giving BM25 the same Atlas source representation. It does not compare all of Atlas against raw file reads, isolate skeleton savings, or simulate a capable model iteratively using `rg` and a language server. BM25 does not intrinsically require embedding preparation even though this shared harness prepares all strategies. The no-expansion comparison is the useful test of whether dependency neighborhoods earn their additional space. Twenty Rust/Go tasks have no supported dependency expansion; language-specific results matter.

There are 50 paired tasks, not 400 independent observations. Two budgets and four methods reuse each task, and multiple tasks reuse repositories. Repository-cluster bootstrap intervals describe uncertainty in this sample, not guaranteed population gains or coding success.

## Operational cost

The final implementation passed every agreed warm latency gate on both fixtures. These are observed fixture results on this macOS machine, not a general service-level guarantee. Each p95 is the maximum of ten samples; no outliers were removed.

| Operation | Target p95 | 10,000-file synthetic | Pinned Django |
|---|---:|---:|---:|
| Dependencies | 0.500 s | 0.248 s | 0.301 s |
| Repository map | 0.500 s | 0.314 s | 0.329 s |
| Semantic search | 2.000 s | 0.355 s | 0.368 s |
| Context assembly | 2.000 s | 1.095 s | 0.678 s |
| Edit through next context answer | 2.000 s | 0.885 s | 1.723 s |

These timings include normal worker requests and source reconciliation after preparation. The deterministic synthetic fixture contains 2,000 files per language and 3,338,612 source bytes. The public fixture is Django commit `8c3bd0b708b488a1f6e8bd8cc6b96569904605be`: 2,803 eligible files, 15,979,072 bytes, with four syntax-invalid/template fixtures explicitly excluded from accepted coverage. Edit trials used an isolated byte-identical copy and restored the inputs.

Cold preparation is a material cost. The observed Django run exceeded a 240-second deadline, saved 1,234 embedding entries, and completed after another 244.834 seconds on restart: about 485 seconds across both semantic preparation attempts, plus graph and restart overhead. The earlier synthetic fresh-cache run took about 57 seconds; that result retains its earlier source provenance. Final scale runs loaded prepared caches and must not be described as entirely cold measurements.

`ATLAS_REQUEST_TIMEOUT` now permits a longer positive, finite request deadline while preserving the 120-second default. The model files were already local; no paid API inference was used. Graph, symbol, skeleton and call tools do not require the embedding model.

Final prepared-cache worker peak RSS reached 0.810 GB for the synthetic fixture and 1.123 GB for Django, with stable persistent cache-entry counts across ten edits. Earlier cold Django inference reached 4.175 GB. These are peak-RSS observations and finite stability checks, not proof of asymptotic memory bounds.

The broader original retrieval run recorded about 19 minutes of embedding preparation for a 5,657-file Ansible snapshot and about 21 minutes for a Material UI snapshot with 26,988 accepted source files. Its recorded setup totals at least 2.47 hours across 33 tasks; the 17 grading failures discarded setup fields, so this is an incomplete lower bound. The original run also reused caches and is not uniformly cold. The corrected run reused 104,769 prepared entries and recorded 334.1 seconds of combined graph/embedding/lexical setup across all 50 tasks: median 3.5 seconds, maximum 27.0 seconds. Cached preparation is much cheaper but still separate from a warm request.

Across the broader retrieval cohort, full Atlas's warm delivery p95 is about 2.20 seconds at both budgets, versus BM25's 2.42/2.50 seconds and flat embeddings' 2.02/2.07 seconds. These timings cover ranking, packing and both MCP renderings; they exclude setup, worker IPC and manifest reconciliation. Several snapshots exceed the 10,000-file scale fixture. Keep this measurement scope separate from the passing worker-scale gates above.

The observed long preparations exceed both the 120-second default worker deadline and a 900-second override. The retrieval harness prepares outside the MCP request deadline, so successful benchmark preparation does not establish that a first MCP semantic query would succeed with the default. Large cold repositories need a suitable server/client deadline or a prepared cache. Persistent reuse strengthens the case for repeated project work; cold preparation weakens the case for a narrow one-off task. Graph, symbol, skeleton and call tools remain usable without the embedding model.

## Engineering result and verification

MCP and CLI use the same repository-session implementation; each session owns a serialized worker with captured source, graph, call index, embeddings and bounded detailed overlays. Metadata/configuration reconciliation is independent of native notifications. Timeouts and active cancellations terminate and join the worker; a subsequent request starts cleanly. Syntax-invalid files leave accepted coverage until repaired, and cursors bind a query to one generation.

The graph keeps distinct Import and SymbolUsage evidence and normalizes PageRank transitions, including dangling nodes. An eager AST call index follows supported lexical/import bindings independently of optional CFG node identities. Python, Rust, JavaScript/JSX, TypeScript/TSX and Go skeletons preserve API material and actual retained source spans. Context packing preserves whole chunks, selected-file priority and complete text/JSON response budgets. Content/model embedding caches are atomic, restartable and quota-bounded.

Verification comprises **313 Rust tests** and a freshly rebuilt release extension, followed by **209 passing Python tests** after the evaluator correction. The original full verifier passed 190 Python tests; the correction added 19 tests and left production source unchanged. No Rust tests were ignored. Verification included actual stdio MCP lifecycle tests and existing CLI/syntax-safety behavior. The first post-correction test invocation omitted the writable task-cache setting, causing four permission failures; the corrected environment passed the full suite in 7.28 seconds, and both logs are preserved. The extension SHA256 is `80a276b6d0bbfba972a408a702515916236ccbdd9c56672f0a61788a511ba3a5`. Dependencies are locked; Python 3.11.14 is the observed environment. CI definitions cover Linux/macOS but have not run remotely.

| Original review failure | Regression coverage now present |
|---|---|
| Edits removed unrelated calls | Incremental/fresh equivalence and call preservation across edit sequences |
| Unknown receivers resolved according to query order | Conservative receiver cases, permutations and qualified ambiguity |
| Unbound locals/parameters acquired unrelated global targets | Lexical shadowing, aliases, context/exception/pattern bindings |
| Skeleton caches served old accepted content | Captured-buffer and cache invalidation checks |
| Selected anchors could not expand | Actual graph context assembly and multi-hop traversal |
| Ordinary edits required explicit MCP refresh | Edit/create/delete/rename/config/error/timeout lifecycle through real MCP |
| Benchmarks credited omitted source | Retained-span validation and actual text/JSON delivery accounting |
| Import edges displaced symbol-use evidence | Typed-pair deduplication, import removal and semantic invalidation |
| Non-Python skeletons retained full bodies | Five-language API/body, large-container and Unicode fixtures |

Independent review also found defects in the new implementation. They were reproduced and fixed: CRLF normalization, queued-cancellation interference, generic handling of timeout errors, JSON cursor pre-parsing, retained cancellation state, metadata displacing selected source, stale semantic edges after local shadowing/export changes, symlink retargets crossing generations, embedding prefixes following a live symlink, and watcher queues acknowledging undelivered events. A 6,000-file burst now eventually delivers all 6,000 persistent changes through the bounded 4,096-event queue and stops promptly under pressure.

Performance fixes followed measured profiles. They removed repeated affected-file traversals, unnecessary accepted-path resolution and repeated/futile metadata tokenization. Exact delivered-source comparisons checked the context optimizations. Earlier failed and contended runs remain in the evidence; targets were not relaxed.

## Practical recommendation

Make targeted structural and API tools the default way to use Atlas when they answer the next question directly. Keep ordinary search, selected body reads and execution-based verification in the workflow. The current comparison supports keeping automatic assembly optional and provides no reason to prefer its full graph-expansion policy over simpler embedding retrieval. Treat dynamic-call omissions and unsupported language analysis as coverage limits. Prefer text unless a consumer needs structured provenance, and use explicit file scope when the task already identifies relevant files.

Before expanding the feature surface, investigate task-specific body selection and delivery overhead on a new held-out sample. The present data does not justify adding more graph layers to automatic retrieval. A separate workflow experiment should compare ordinary search/read access with Atlas-enabled access under the same model, tasks and prompts, logging all tool costs and scoring completed changes with held-out tests and review. Compare usage at comparable correctness and retain unsuccessful runs in the accounting. That experiment was not performed here, so overall agent-token and coding-accuracy gains remain unmeasured.

## Recovery and evidence

The original April work is preserved in `357ae6b`; the accepted plan is in `7680296`. Implementation commits are `0892fb3` (Rust graph/calls/skeletons), `2ed3384` (sessions/context/caches/verification), `a796d49` (frozen comparisons and scale harnesses), and `d90e59d` (measured packing fixes and cold deadline). The evaluator correction and regressions are in `0580d7b`. Final report/evidence are recorded in a subsequent local documentation commit. Original untracked historical documents and editor files remain untouched.

Start a future session with `IMPLEMENTATION_STATE.md`, this report, and `IMPLEMENTATION_PLAN.md`. The evidence package contains the frozen manifest, raw results, paired analysis, scale report, passing verifier log, relevant earlier failures and exact reproduction instructions. It excludes model binaries and public checkouts.

Remaining technical limits include conservative dynamic resolution; no Rust/Go import or call resolver; no established Python module resolution through filesystem symlink aliases; skeletons that are descriptive rather than compilable; and existing Rust/PyO3 warnings. This was not an exhaustive audit of every CFG/data-flow path. Python 3.10 is permitted by package metadata but was not the verified runtime.
