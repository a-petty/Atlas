The corrected experiment does not establish a useful retrieval advantage for Atlas’s graph expansion or PageRank anchor promotion over flat embeddings. Atlas has some favorable aggregate comparisons with BM25, but they vary sharply by language and the repository-cluster uncertainty intervals are wide. I recommend targeted structural MCP tools as the default value proposition and automatic context assembly as an optional aid. This experiment measures retrieval; it does not demonstrate better coding-task completion or lower total agent-token use.

The run completed all 50 pinned tasks (10 each in Python, JavaScript, TypeScript, Rust, and Go), with four methods at two budgets: 400 measurements, zero execution errors, zero token/character budget violations, and identical accepted-file hashes across methods. All methods use the same eligible source set, skeleton-derived embedding/lexical representation, map/full-source/skeleton packer, query allowance, and actual MCP rendering. No explicit files were supplied to assembly. The comparison therefore isolates automatic retrieval choices within that shared delivery system; it does not compare Atlas with an agent using targeted calls, plain search, or whole-file BM25.

Primary quality means retain all 50 tasks. File recall is the share of annotated file identities delivered; file F1 also penalizes unrelated delivered files. Known-byte recall measures actual retained annotated source bytes, not whole-file credit for returning a skeleton label.

| Budget | Method | File recall | File F1 | Known-byte recall | Text tokens | JSON tokens |
|---:|---|---:|---:|---:|---:|---:|
| 8,000 | BM25 | 16.90% | 12.69% | 3.61% | 3,632.9 | 6,681.4 |
| 8,000 | Flat embeddings | 18.36% | 14.06% | 7.29% | 3,457.6 | 6,675.5 |
| 8,000 | Atlas, expansion disabled | 17.94% | 13.49% | 7.29% | 3,484.4 | 6,676.6 |
| 8,000 | Atlas | 17.94% | 13.49% | 7.29% | 3,485.4 | 6,678.4 |
| 12,000 | BM25 | 18.10% | 13.37% | 8.04% | 5,910.6 | 10,688.7 |
| 12,000 | Flat embeddings | 20.01% | 14.16% | 10.01% | 5,949.1 | 10,683.3 |
| 12,000 | Atlas, expansion disabled | 21.05% | 13.63% | 9.94% | 5,940.8 | 10,679.1 |
| 12,000 | Atlas | 21.05% | 13.66% | 9.92% | 5,939.5 | 10,678.7 |

Paired differences below are Atlas minus the named comparator, in percentage points. Intervals resample the 19 distinct repository URLs, keeping paired tasks from the same repository together (10,000 resamples; seed 20260905). Repository URLs merge dataset aliases such as `mui/material` and `mui/material-ui`. These are descriptive cluster sensitivity intervals, not confirmatory significance tests: language quotas are not preserved in resamples, and there is no multiple-comparison adjustment.

| Budget | Comparator | Δ file F1 | 95% cluster interval | Δ known-byte recall | 95% cluster interval |
|---:|---|---:|---|---:|---|
| 8,000 | BM25 | +0.798 | [-11.987, +10.239] | +3.672 | [-2.364, +12.190] |
| 8,000 | Flat embeddings | -0.570 | [-2.761, +1.138] | +0.000 | [+0.000, +0.000] |
| 8,000 | Atlas, expansion disabled | +0.000 | [+0.000, +0.000] | +0.000 | [+0.000, +0.000] |
| 12,000 | BM25 | +0.289 | [-10.891, +9.472] | +1.881 | [-9.826, +13.163] |
| 12,000 | Flat embeddings | -0.501 | [-3.376, +1.455] | -0.087 | [-0.557, +0.440] |
| 12,000 | Atlas, expansion disabled | +0.035 | [-0.122, +0.183] | -0.016 | [-0.063, +0.000] |

At 8k, expansion changes neither quality metric on any of the 50 tasks. At 12k, its file-F1 comparison has 2 wins, 47 ties, and 1 loss; byte recall has 49 ties and 1 loss. Compared with flat embeddings, Atlas’s file F1 is lower at both budgets, with identical byte recall on all tasks at 8k and a small negative mean byte difference at 12k. The [0, 0] intervals describe equality in this sample, not proof of equality on future tasks.

Only 32 tasks have nonempty gold, positive measurable bytes, and complete span metrics. Their companion estimates use 14 repository clusters and preserve the primary results separately. On this subset, Atlas minus BM25 file F1 is −1.904 pp at 8k and −2.449 pp at 12k, while known-byte recall is +2.943 pp and +0.026 pp. The broad intervals still cross zero. The modest favorable all-50 BM25 comparison is therefore not a stable general advantage; subset composition matters.

| Language (10 tasks each) | 8k ΔF1 vs BM25 | 8k Δbytes vs BM25 | 12k ΔF1 vs BM25 | 12k Δbytes vs BM25 |
|---|---:|---:|---:|---:|
| Python | -25.110 | -3.869 | -21.110 | -19.743 |
| Javascript | +1.316 | -1.644 | +1.615 | -1.644 |
| Typescript | +9.199 | +2.139 | +5.510 | +6.359 |
| Rust | +5.970 | +1.711 | +1.280 | +0.588 |
| Go | +12.613 | +20.022 | +14.150 | +23.846 |

Python is the strongest counterexample to a broad Atlas accuracy claim: BM25 file F1 is 33.75% at both budgets, compared with Atlas’s 8.64% and 12.64%. Much of Atlas’s favorable aggregate byte result comes from Go. These are small, repository-dependent language groups, not independent estimates from ten unrelated projects.

The following examples are deliberately selected extremes after observing the sample, not representative success-rate estimates. On Go CLI task `…3d1b3145`, Atlas delivers `pkg/cmd/pr/status/status.go` and 100% of measurable annotated bytes at both budgets; BM25 delivers none. At 12k on Django task `…1c5aa714`, BM25 delivers `django/db/migrations/executor.py` with 100% byte recall, while Atlas misses it. On Django `…986c9e85`, BM25 achieves file F1 of 1.0 for `django/forms/widgets.py` while Atlas misses the file, yet both retain zero annotated bytes. The last case shows why file hits cannot substitute for source-span coverage.

Actual response costs provide little support for an Atlas-specific token saving. Against BM25, Atlas saves 147.5 text tokens (4.06%) at 8k, but only 3.0 JSON tokens (0.045%); at 12k it uses 28.9 more text tokens and 10.0 fewer JSON tokens. Against flat embeddings, it uses 27.8 more text tokens at 8k and saves only 9.6 at 12k. Expansion’s text-token changes are +1.0 and −1.3 tokens. All paired token comparisons include 50 successful responses. The common compression/packing mechanism is shared by the baselines and cannot be credited to Atlas’s ranking. These counts use `cl100k_base`; they are neither provider-billed tokens nor total coding-session usage, and lower token counts with missed relevant source are not demonstrated useful savings.

Warm Atlas delivery p95 is approximately 2.20 seconds at both budgets, versus BM25’s 2.42/2.50 seconds and flat embeddings’ 2.02/2.07 seconds. These measurements include ranking, packing, and rendering both MCP formats; they exclude model/graph setup, process IPC, and manifest reconciliation. A real client selects one response format. The separate scale fixtures measure different operations/workloads and should retain their own stated scope.

Gold measurability and source eligibility place real limits on interpretation. There are 522,819 known annotated bytes and 49 unmeasurable gold blocks across 16 tasks. Forty-seven tasks have positive measurable bytes; the other three remain in the primary denominator. Empty gold occurs on JavaScript `Multi-SWE-Bench__javascript__maintenance__bugfix__3cd96c13` and Rust `Multi-SWE-Bench__rust__maintenance__bugfix__0e4e102e`. Rust `Multi-SWE-Bench__rust__maintenance__bugfix__b613bda3` contains only external scratch-file annotations. These three supply no observed byte-recall evidence; they are excluded from the explicitly labeled complete-gold companion subset, not replaced with new tasks.

The original dataset has absolute paths on 17 tasks, including verified repository-container paths on 16. The corrected grader records 71 container-prefix path mappings and 7 invalid path identities, while preserving original paths and canonical IDs. External scratch annotations remain in file denominators and invalid/missed block counts; their byte sizes are unknown. Nothing outside a pinned checkout was read.

Accepted source IDs were reconstructed exactly against the recorded hash/count for 39 of 50 tasks. Eleven could not be reconstructed because invalid-source diagnostics were truncated; those checks remain unknown, not passed. Delivery by some arm provides additional acceptance evidence. Across 242 task-file occurrences, 49 are known excluded: 36 unsupported extensions, 6 excluded-directory paths, and 7 unresolved annotation paths. Fourteen gold file occurrences remain of unknown eligibility. Known exclusions account for 43,355 of 522,819 known bytes (8.29% by byte mass). With the primary 50-task zero convention retained, the optimistic macro recall ceilings from known exclusions are 79.86% for files and 82.23% for known bytes. These are upper bounds, not adjusted scores; unknown exclusions could lower them.

The source ceiling cannot explain most of the weak recall. At 12k, Atlas still misses 147 proven accepted gold-file occurrences; 15 gold blocks are only partially retained. In the first Ansible task, excluded `lib/ansible/plugins/filter/strftime.yml` accounts for only 333 of 44,431 known bytes. Its known-byte ceiling is 99.25%, while Atlas retains 1.24% at 8k and misses four other proven accepted gold files. The saved evidence distinguishes source-policy exclusions from selection misses and incomplete retention, but generally cannot separate ranking failures from packing failures.

Preparation costs must be counted once per task, even though each timing appears on eight method/budget rows. The corrected rerun began with a prepared cache of 104,769 NPZ files (549,433,609 bytes), so its setup costs are not cold-start measurements. The original run already included public-cache reuse and reuse across commits, so it also is not a uniformly cold sample. Original grading failures discarded setup fields for 17 tasks; its reported totals cover only 33 tasks and are incomplete lower bounds.

| Run / setup stage | Tasks timed | Total seconds | Median seconds | p95 seconds | Maximum seconds |
|---|---:|---:|---:|---:|---:|
| Corrected, prepared cache: graph | 50 | 204.8 | 2.3 | 14.6 | 16.5 |
| Corrected, prepared cache: embeddings | 50 | 91.9 | 1.2 | 5.7 | 7.1 |
| Corrected, prepared cache: lexical | 50 | 37.4 | 0.3 | 3.4 | 3.4 |
| Corrected, prepared cache: combined | 50 | 334.1 | 3.5 | 22.7 | 27.0 |
| Original, mixed reuse: graph | 33 | 177.3 | 4.0 | 15.3 | 16.7 |
| Original, mixed reuse: embeddings | 33 | 8,667.0 | 83.6 | 1,142.4 | 1,249.6 |
| Original, mixed reuse: lexical | 33 | 35.2 | 0.7 | 3.5 | 3.5 |
| Original, mixed reuse: combined | 33 | 8,879.5 | 84.7 | 1,160.8 | 1,269.7 |

The original run’s measured setup alone totals at least 2.47 hours across 33 tasks, dominated by 2.41 hours of embedding preparation. Worst recorded combined preparations were MUI `…4c3f5d3d` at 21.16 minutes, Ansible `…83c269c9` at 19.35 minutes, MUI `…676e9486` at 17.13 minutes, and VS Code `…16d1ff7a`/`…7d106697` at 14.74/11.02 minutes. The prepared-cache rerun totals 334.1 setup seconds across all 50 tasks; its worst task still needs 27.03 seconds. Checkout timing is separate: 5.7 seconds in the corrected run and 10.6 seconds in the 33 recorded original tasks. Embedding setup was shared by the experiment but is not an intrinsic BM25 requirement. The lexical preparation followed embeddings and could benefit from already prepared source representations.

The evidence checks passed with one limited unresolved consistency discrepancy. The recorded implementation fingerprint `0e6413797e2fff31ae4bb8f164c841953b7a4ead3f8904b621ff9041e50b9e93` is internally valid and exactly matches the current 49 source/extension/lockfile labels and selected dependency versions. Dataset hash, pinned clean checkouts, recorded normalization mappings, and all known-byte totals were replayed successfully. Model identities agree across all rows. Fingerprints record code/environment provenance, not every host setting or immutable filesystem history.

All 264 originally successful task/method/budget rows have identical delivered-file lists, text/JSON token counts, accepted-file counts/hashes, and engine keys in the corrected run. The original 136 grading-error rows have no comparable saved retrieval measurements. Only `benchmarks/bench_retrieval.py` changes in the source fingerprint. Four 8k rows for `SWE-PolyBench__typescript__evolution__feature__d520ba22` have text and JSON character counts lower by exactly 2. Coverage and omission counts are unchanged; all eight rows for that task also have identical gold bytes, retained bytes, full/partial/missed block counts, and quality metrics. The exact text was not saved, so the character discrepancy remains unresolved. This supports retrieval consistency but does not establish byte-identical responses; latency and legitimately changed gold accounting were not required to match.

The practical case for Atlas is therefore explicit source navigation, structural summaries, and controlled context delivery with the separately tested freshness/runtime guarantees. This retrieval experiment does not establish that automatic graph-expanded assembly should replace simpler retrieval or an agent’s targeted tool use. It especially does not establish a default advantage for Python work. No coding-agent completion trial, avoided-file-read measurement, or total-token comparison was performed.

Evidence: `retrieval-results-corrected.json`, `retrieval-results-corrected.analysis.json`, `retrieval-results-before-path-correction.json` (the original work filename is `retrieval-results-final.json`), `corrected-run-provenance.json`, and `retrieval-analysis.log`. Reproduction uses the locked virtual environment and `analyze_retrieval_results.py` with the pinned dataset/checkouts, `--atlas-repo /Users/apetty/Dev/Atlas`, and `--original-result retrieval-results-before-path-correction.json` within the evidence package. All source and ranking code remained frozen during analysis.
