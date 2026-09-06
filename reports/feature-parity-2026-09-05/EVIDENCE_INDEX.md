# Atlas evidence package

Read `IMPLEMENTATION_REPORT.md` first. It answers the capability, token and accuracy questions, then records engineering verification and operating cost.

- `retrieval-results-corrected.json` contains the frozen 50-task measurements, run configuration and source/model fingerprints. `retrieval-results-corrected.analysis.json` contains paired, per-language and preparation summaries. The 400 rows reuse 50 tasks and are not independent trials.
- `scale-validation-report.md`, `scale-accepted.json` and `public-scale-accepted.json` contain the final passing warm measurements. Earlier cold and failing results retain their original filenames and provenance.
- `final-verification.log` records the final production Rust/build/Python verification. `post-grader-python.log` records the full Python suite after the evaluator-only correction. `review-closure.log` records independent reproductions after fixes.
- `BENEFIT_CLAIMS_REVIEW.md` independently examines what these measurements can establish. The other review reports describe implementation defects and their closure; earlier-stage recommendations in those reports must be read with the final report.
- `contextbench_50_v1.json`, `benchmark-checkouts.json`, the public source manifest and `cache-provenance.json` record the sample, source and cache identities.
- `REPRODUCE.md` provides commands and limitations. `analyze_retrieval_results.py` derives the analysis from the completed raw result.
- `retrieval-results-before-path-correction.json` preserves the original evaluator failures. The grader correction and independent review explain the schema-3 gold-path policy; `corrected-run-provenance.json` records the rerun's source and cache reuse.
- `tool-catalog-accounting.json` contains the actual MCP tool descriptors and compact-JSON token count. `measure_tool_catalog.py` reproduces that limited measurement; it does not measure client prompt or billed usage.
- `skeleton-token-illustration.json` and its note preserve one deliberately selected source/skeleton comparison, including the debugging checks the compressed view omits. It is separate from the frozen benchmark and is not a general savings estimate.
- `IMPLEMENTATION_PLAN.md` and `IMPLEMENTATION_STATE.md` preserve the accepted scope and recovery state.

`EVIDENCE.sha256` fingerprints the bundled files. The package intentionally excludes model binaries, virtual environments, build caches and public repository checkouts. No paid coding-agent outcome is represented by these retrieval results.
