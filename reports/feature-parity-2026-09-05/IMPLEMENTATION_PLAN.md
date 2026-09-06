# Atlas full feature parity implementation

Accepted by Alec on 2026-09-05. Branch: `codex/atlas-feature-parity`.

## Agreed outcome

Full current-README feature parity, automatic freshness, approximately 10,000-source-file monorepos, and a reproducible retrieval comparison. Parsing, symbol extraction and real compression cover Python, Rust, JavaScript/JSX, TypeScript/TSX and Go. Import resolution covers Python and JS/TS; conservative CPG analysis covers Python. Preserve CLI commands and the Rust/Python boundary. New import resolvers, additional CPG languages, model providers and a coding-agent completion trial are outside scope. Planning estimate was 50–80 agent-hours, subject to evidence from the fresh baseline.

## Fixed design decisions

1. **Baseline:** preserve April dirty/untracked work separately; build the real extension and verify its import path. Lock Python dependencies and provide a single fresh-build verification command. Use Python 3.11 and the existing Rust toolchain initially. Keep build caches task-local.
2. **File edges:** allow one Import and one SymbolUsage edge per ordered file pair. Preserve existing labels and tuple APIs. Deduplicate typed edges; remove semantic evidence when its supporting import/binding disappears. PageRank weights remain 2 and 1 with normalized transitions and dangling-node handling. Traversal deduplicates paths but preserves evidence; processed anchors must still expand.
3. **Call graph:** AST-extracted declarations/scopes/bindings/call sites use stable generation-scoped identities independent of CFG NodeIndex. CFG/reaching definitions are lazy. All analysis modes share conservative lexical/import resolution. Respect parameter/local shadowing, import aliases and re-exports; unknown receivers remain unresolved. Index unresolved potential dependencies and old/new exports for invalidation. Refresh affected callers' complete call sites without removing unrelated incoming/outgoing relationships. Ambiguous function queries return qualified candidates; implementation/stub preference applies only to the same qualified callable.
4. **Freshness:** shared repository session for MCP and CLI; one worker process owns mutable state, serialized requests, hard timeouts terminate/join before replacement. Watcher hints plus pre-request manifest/config reconciliation capture edits missed by events. Publish coherent content-hashed generations; answer only from accepted source buffers. Changes during a response enter the next generation. Cache keys include content and engine/model identity. Per-file syntax invalidity preserves usable local views and exposes incomplete coverage; whole-generation failures return unavailable. Persist embeddings outside the checkout and reuse unchanged content. Initial semantic preparation reports progress rather than a partial ranking presented as complete.
5. **Skeletons:** Tree-sitter extraction produces original retained spans plus placeholders. Preserve signatures, imports/exports, documentation, attributes/decorators, fields/constants, nested declarations, Rust impl/traits, Go methods/interfaces, JS arrow/export forms, TS declarations and Python stubs. Compression is a descriptive view, not required to compile.
6. **Context:** restore repository map (up to 8%), adaptive full source (40–75% of remainder) and skeleton tiers. Priority is explicit files, anchors, neighbors, then extended architecture. Internal structured results carry generation, source spans, selection/mode, budgets and omissions. Text and benchmark rendering use that result. Preserve text API; optional JSON, budget and explicit-file parameters. Complete rendered response stays within 60,000 characters including metadata. Whole chunks only, skeleton fallback for oversized full files, explicit omissions. Large lists paginate with generation-bound cursors.
7. **Evaluation:** freeze 50 new ContextBench tasks, 10 each Python/JS/TS/Rust/Go, excluding the prior ten. Pin dataset, IDs, repo commits and seed. Compare BM25, flat embeddings, Atlas without expansion and full Atlas under equal representation, eligible files, 8k/12k token budgets and transport constraints. Measure actual retained gold spans, file P/R/F1, tokens, cold/warm latency and failures/incomplete coverage. Report unfavorable outcomes honestly; coding-task completion is not claimed.

## Stages and acceptance

- [x] 1. Baseline and regression integration: fresh-source provenance and full baseline; nine review probe groups converted to desired-behavior assertions.
- [x] 2. Graph/call correctness: typed-edge/traversal tests, conservative resolution, incremental/fresh equivalence and query-order permutations.
- [x] 3. Automatic freshness: actual MCP edit/create/delete/rename/config/error/timeout lifecycle, coherent cached results without refresh.
- [x] 4. Language/context parity: API-preserving skeleton fixtures, structured result and exact delivered spans/budgets.
- [x] 5. Scale/retrieval: deterministic 10k fixture plus pinned public repo, frozen comparative sample and reproducible results.
- [x] 6. Release preparation: updated docs/build/verification/recovery notes and independent adversarial review.

Tests include import removal, unresolved-to-resolved imports, re-export changes, shadowing, ambiguous methods, unrelated call preservation, API/cache edits, syntax repair, lost events, query cancellation, CFG eviction, Unicode/large chunks, pagination, and existing CLI/syntax-safety behavior. Property-based edit sequences compare canonical final state with a fresh build.

Performance targets after preparation: p95 dependency/map <=500ms; warm search/context <=2s; ordinary single-file edit through next answer <=2s. Report cold stages separately. Repeated cycles must have bounded memory/cache growth. Targets missed remain release issues and are not silently lowered.

## Working protocol

One owner integrates graph/session interfaces. Independently owned language and benchmark work can run after interfaces are fixed. Each stage leaves verified commits and a current state note; continue authorized stages without requiring another nudge. After two failed fixes, switch to diagnosis and obtain evidence. No pushes, deployments, client-system writes or paid model evaluations.

Recovery archive and original review evidence are in `/Users/apetty/Documents/Codex/2026-09-05/i/work/implementation/` and `/Users/apetty/Documents/Codex/2026-09-05/i/outputs/atlas-evaluation/`. Original source archive excludes `.git`, venv, compiled extension and build artifacts; original status and tracked diff are saved separately. The source repository started at `8a1962e5cb7fd7603a544b9f0687a130d852ab40`.
