# Skeleton implementation and independent delivery review

2026-09-05. Subagent owns parser/skeleton/query files; no commit or push performed.

## Implemented and verified

- Structured source-attributed skeleton text/chunks/spans; original UTF-8 byte ranges and 1-based physical LF lines, placeholders uncredited.
- Actual body omission for Python/.pyi, Rust, JS/JSX/mjs/cjs, TS/TSX, Go.
- Documentation, decorators, attributes, fields, constants, types, imports/exports, nested declarations, conditional Python API, Rust impl/traits/macros/modules, Go receiver/interface signatures, JS arrows/CommonJS and TS declarations.
- Large container skeletons split at member boundaries with original enclosing headers and closing braces. Large initializer payloads omitted atomically. Adjacent source spans coalesced.
- Rust/Go symbol harvesters use their language queries; TSX gets its own grammar query; TypeScript declaration-only and abstract APIs harvested.
- Parser new JSON function registered by parent. Text functions preserve compatibility; unsupported-language fallback remains exact source.

Verification: 12 new language/span/chunk tests, 4 existing phase2 criteria tests, 7 existing symbol-harvester tests passed against freshly compiled Rust source. Log: `work/skeleton-tests.log`. The old phase2 test explicitly dropped GLOBAL_VAR; updated to the accepted API-preservation contract. Its signature-heavy fixture now reduces tokens 32.7%; minimum adjusted to25%. A separate 1000-statement fixture verifies >95% body compression.

## Independent review of context and graph delivery

Claims tested: requested-file priority, actual source attribution, complete-response limits, and genuine graph expansion without broad retrieval fallback.

Confirmed findings (desired-behavior regressions at `work/test_context_adversarial_regressions.py`):

1. Higher-priority explicit skeleton can be displaced by unrelated full files. At max_tokens2000, an explicit large-body/400-token API file is included alone; with15 low-priority small files it is omitted and all15 unrelated files are delivered. Cause: full-source pass admits lower-priority files before any skeleton fallback. Context worker reports staged fix: single ranked pass with immediate full-to-skeleton fallback. Not independently reverified after integration yet.
2. Full-source end_line uses Python splitlines, counting U+2028 inside a string as a physical source line. Two-LF-line source is attributed through line3. Context worker reports staged LF-count fix. Not independently reverified after integration yet.
3. Aliased imports lose SymbolUsage edges. Real chain a->b->c loses c in direct graph traversal when b imports `gamma as renamed` and calls renamed. Unaliased control produces Import+SymbolUsage; aliased case has Import only. Root informed; unresolved at review snapshot.
4. The inverse binding error produces unsupported SymbolUsage: `from b import unused; def run(foo): return foo()` points to b.foo despite the parameter call and unrelated imported name. Root informed; unresolved at review snapshot.

Budget question requiring implementation choice: source-text token counts exclude JSON manifest tokens. Probe max_tokens2000 delivers text836 tokens but JSON2130 tokens. Character ceiling is checked for both representations. Root informed; output-format-aware token accounting recommended if the token budget is meant to cover the full tool response.

Limits: graph probes use the actual installed Rust extension and ContextManager but do not go through an MCP client; context priority probe uses accepted-source graph double and deterministic skeleton double to isolate packing from search/parsing. Integrated MCP response tests remain parent responsibility. No claim that all acceptance criteria pass.
