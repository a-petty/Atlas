# Frozen grader diagnosis and staged correction

The original schema-2 evaluator does not normalize dataset container paths. `parse_gold_context` preserves each `file` literally; `root / absolute_file` discards the checkout root, and `retained_span_metrics` raises `ValueError: Gold path escapes repository` before appending a measurement. This is an evaluator/annotation compatibility defect, not evidence that Atlas delivered outside-repository content.

A lightweight raw-path census of the unchanged 50-task manifest found 17 tasks containing absolute gold paths: JavaScript 6, Go 4, Rust 6, TypeScript 1, Python 0. No gold paths contained parent traversal components. The original run and all its error rows remain unchanged.

Observed examples:

- Svelte task `Multi-SWE-Bench__javascript__maintenance__bugfix__00fd24ea`: 13 blocks and 5 files under `/workspace/sveltejs__svelte__0.1/`. All repository-relative suffixes exist in the pinned checkout.
- GitHub Readme Stats task `Multi-SWE-Bench__javascript__maintenance__bugfix__0ed136bf`: 9 blocks and 6 files under `/workspace/anuraghazra__github-readme-stats__0.1/`. The manifest's `repo` label is `anuraghazra/github`, so the trusted `repo_url` is needed to verify the actual repository name. All suffixes exist in the pinned checkout.
- Svelte task `...__0ed92fe3` also contains external scratch annotations `/workspace/ast_parsed.json`, `/workspace/compiled.js`, and `/workspace/reproduce.mjs`. These cannot be treated as repository files by removing an arbitrary prefix.

The correction is staged only, pending parent integration after the original run completes:

- `work/evaluator-overlay/benchmarks/bench_retrieval.py`
- `work/evaluator-overlay/python_shell/tests/test_retrieval_benchmark.py`
- Analysis compatibility: `work/analyze_retrieval_results.py`

Schema 3 records the explicit `verified-repository-container-prefix-v1` gold policy in run configuration and every result row. Resume rejects different schema, source fingerprint, or gold-policy identities. The manifest sample, model, ranking, budgets, and packing remain unchanged. All 50 tasks must be rerun to a new result output; no previous measurements are substituted or silently repaired.

For a recognized absolute container path, normalization uses the owner/repository from the pinned public GitHub URL, accepts a syntactically valid opaque container version label without an ambiguous `__` separator, and verifies that the suffix resolves to an existing file inside the pinned checkout. The pinned commit, not that container label, determines source content. Original paths, mapped metric IDs, canonical paths, prefixes, and invalid reasons are recorded. Wrong-owner/repository prefixes, scratch paths, missing paths, malformed paths, parent traversal, and external symlink targets remain explicit invalid annotations. No outside source is read.

File recall retains distinct canonical repository-relative IDs plus original unresolved path IDs in its denominator; mapped aliases to the same file count once. Unsupported source files and external scratch files are not dropped. Byte recall continues to use the union of measurable base-source line ranges; invalid paths/ranges have unknown byte sizes, remain explicitly listed, and count as missed blocks. A task with no measurable bytes has no observed byte-recall evidence, even though the primary metric uses zero by convention.

Validation completed without model initialization or repository indexing: 37 pure benchmark/path/accounting/resume tests passed in 0.48 seconds. One integration test requiring the real capture/graph/packer is intentionally deferred to the parent verifier. The analysis replay fixture also passed for normalized bytes, unknown external scratch bytes, and refusal of tampered recorded mapping provenance. Full analysis uses the locked virtual environment and verifies exact implementation/dependency fingerprints before source-policy replay.

Independent review found and corrected an alias-identity defect in the initial staged grader: `a.js`, `./a.js`, and an internal symlink previously counted the same source as separate IDs and tripled its gold-byte denominator. All valid paths now use `candidate.relative_to(root).as_posix()` after canonical resolution; original paths and container prefixes remain separate provenance. A four-alias regression verifies one file identity, one 6-byte union, and full byte recall. The ambiguous `org__repo__other__0.1` prefix is also explicitly rejected so the version label cannot conceal an additional repository-name segment.

The raw-path census qualification is 17 absolute-path tasks, of which 16 contain repository-container paths. Rust task `...__b613bda3` is scratch-only; JavaScript `...__3cd96c13` and Rust `...__0e4e102e` have empty gold. The analysis preserves all primary denominators, lists empty/zero-measurable-gold tasks, and requires nonempty files plus positive measurable bytes for complete-gold companion subsets.

The final analysis supports `--original-result ORIGINAL_COMPLETE_JSON`. After both runs finish, this compares original successful rows with corrected successful rows at identical task/method/budget keys using saved delivered-file lists, text/JSON token and character counts, accepted-file counts/hashes, and actual engine keys. It reports differences, absent fields, and new failures; original error rows are explicitly excluded. Manifest/model/dependency identity and source-fingerprint changes are also reported. Latency and gold-accounting changes are permitted. Synthetic checks passed for identical delivery metadata despite changed latency/gold metrics, plus detection of altered token counts and missing fields. This provides observed consistency evidence; saved rows do not include full response text, so it cannot prove byte-identical responses.
