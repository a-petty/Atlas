# Independent evaluator path review

Date: 2026-09-05. Scope: the staged evaluator correction in `work/evaluator-overlay/benchmarks/bench_retrieval.py` and its staged `python_shell/tests/test_retrieval_benchmark.py`, compared with the live Atlas versions and `gold-absolute-path-diagnosis.json` / `gold-path-form-census.json`.

The correction is appropriate for the frozen annotation-path problem. It changes gold-path interpretation and evaluation provenance, without changing retrieval ranking or packing. Preserve the old run as failed-evaluator evidence and rerun the entire unchanged frozen sample under schema 3. Do not combine old and corrected rows or replace tasks.

## Confirmed defect found in the first staged correction

The first version resolved annotation paths canonically but kept their original relative spellings as metric IDs. In a temporary fixture containing `a.js` with `value\n` and `alias.js` pointing to `a.js`, annotations `a.js`, `./a.js` and `alias.js` all referred to the same six source bytes. Delivering those bytes under canonical `a.js` nevertheless produced:

```text
files: ["a.js", "./a.js", "alias.js"]
canonical paths: ["a.js", "a.js", "a.js"]
gold_bytes: 18
retained_gold_bytes: 6
gold_byte_recall: 0.3333333333333333
blocks_fully_covered: 1
blocks_missed: 2
invalid_gold_blocks: []
span_metrics_complete: true
```

This was a real accounting defect, not a path-escape issue. The author corrected valid metric IDs to `candidate.relative_to(root).as_posix()`, preserving original paths and container prefixes separately as provenance.

The initial version also accepted `/workspace/org__repo__other__0.1/a.js` for pinned `org/repo`, because its opaque version segment allowed the `__` delimiter. That directory name can also represent repository `repo__other` at version `0.1`. The author now rejects `__` inside the version segment. No such ambiguous version was found in the frozen-path census; this was identity hardening, not an observed score effect in the fifty tasks. It never allowed a filesystem escape.

## Independent recheck of the corrected staging

The reviewer extracted only the pure gold-normalization, provenance and metric functions from the staged Python AST and exercised temporary files. No embedding/model initialization, repository indexing, network calls or live-source edits occurred. The complete evaluator test suite was not rerun by this review.

The alias check used four annotations: `a.js`, `./a.js`, internal symlink `alias.js`, and `/workspace/org__repo__0.1/alias.js`. All normalized to the same canonical `a.js`. The exact observed output was:

```text
ALIASES_PASS {"gold_bytes": 6, "retained_gold_bytes": 6, "gold_byte_recall": 1.0, "blocks_fully_covered": 4, "blocks_partially_covered": 0, "blocks_missed": 0, "invalid_gold_blocks": [], "span_metrics_complete": true}
BOUNDARY_PASS 9 invalid paths retained
PINNED_URL_IDENTITY_PASS
UNCHANGED_AST_PASS: BM25, freeze, checkout, terms, summary, full ranking/packing/repeat loop
```

The nine rejected boundary paths were:

1. `/workspace/org__repo__other__0.1/a.js`
2. `/workspace/other__repo__0.1/a.js`
3. `/workspace/org__wrong__0.1/a.js`
4. `/workspace/reproduce.py`
5. `../a.js`
6. `src/../a.js`
7. `/workspace/org__repo__0.1/../a.js`
8. `/workspace/org__repo__0.1//a.js`
9. `/workspace/org__repo__0.1/outside.js`, where `outside.js` was a symlink to a path outside the temporary repository.

Each retained an invalid reason, had no accepted source path, and counted as a missed block. A task with abbreviated `repo='org/abbreviation'` but pinned `repo_url='https://github.com/org/repo.git'` correctly accepted the `org__repo` container prefix. Prefix validation uses the pinned repository URL rather than a basename guess; the version label is syntax-checked, while source identity is separately tied to the pinned checkout commit.

## Ranking and provenance review

AST equality checks against the live runner passed for the `BM25` class and the `frozen_manifest`, `pinned_checkout`, `terms`, and `summarize` functions. The entire `for _ in range(repeats + 1)` block in `evaluate` was also AST-identical. That block performs candidate ranking, context packing, text/JSON rendering and repeat timing. The staged diff makes no model, ranking-weight, packing-policy, budget, manifest or strategy change.

This establishes unchanged implemented ranking/packing logic; it is not a claim that a new full run has already produced identical timings or outputs. The old evaluator aborted affected tasks during grading, so corrected measurements must come from the new run.

Schema 3 includes the gold-path policy in run configuration and records the policy ID per row. Resume validation compares the complete configuration and row policy identity; existing implementation fingerprints also change with the evaluator code. Original paths, canonical mappings, container prefixes and invalid reasons remain auditable in successful rows.

Distinct canonical valid paths count once in the file denominator. Unresolved, missing, unsupported and external annotations remain as misses rather than being silently removed. Byte recall unions valid line intervals in verified local files. Unknown bytes for invalid paths/ranges cannot be added to that denominator; they are listed explicitly and counted as missed blocks. Consequently, known-byte recall must be accompanied by invalid-block and measurement-completeness information.

## Corrected frozen-sample census

The census contains **17 tasks with absolute annotation paths**, but **16 tasks with matching repository-container paths**. The remaining absolute-path task is:

```text
Multi-SWE-Bench__rust__maintenance__bugfix__b613bda3
/workspace/reproduce_issue/Cargo.toml
```

It contains only an external scratch annotation and cannot be repaired by stripping a verified repository prefix.

Two additional tasks contain zero gold files in the parsed census:

```text
Multi-SWE-Bench__javascript__maintenance__bugfix__3cd96c13
Multi-SWE-Bench__rust__maintenance__bugfix__0e4e102e
```

Retain all fifty planned tasks and disclose these cases separately. Do not describe the sample as fifty fully measurable source-context tasks, and do not interpret zero/unknown gold-byte scores as evidence of a model's coding accuracy. These annotation limitations apply equally to all four retrieval methods.

## Conclusion

The identified alias accounting defect and prefix ambiguity are closed in the reviewed staging. The remaining path-boundary, denominator and provenance behavior is consistent with the documented correction. No further concrete grader defect was found in this bounded review. The full corrected run and its aggregate results remain the parent task's responsibility.
