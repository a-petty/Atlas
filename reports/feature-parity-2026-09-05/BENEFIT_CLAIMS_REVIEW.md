# Independent review of Atlas benefit claims — 2026-09-05

Atlas currently has a stronger case as a repository navigation and representation service than as a proven accuracy multiplier. It gives a model convenient access to precomputed structure and compact source views. It does not provide repository facts that are fundamentally inaccessible through source inspection. Whether the convenience reduces total tokens or improves completed work remains an empirical question.

This review inspected implemented interfaces and evaluation code while the frozen fifty-task comparison was running. It makes no claim about that run's unfinished results and ran no model/coding trial. No source files changed.

## What the model gains

| Capability | Concrete benefit | Comparison and practical limit |
| --- | --- | --- |
| Typed dependencies, dependents, callers and callees | One targeted query can replace repeated name searches, import tracing and manual filtering. The resolver respects supported lexical shadowing and aliases instead of selecting unrelated matching names. | All evidence comes from source. A model can recover it with reads and reasoning, although doing so repeatedly is costly. Existing equivalent language-server tools reduce the incremental benefit. Dynamic calls remain unresolved; empty output is not proof of no runtime dependency. |
| Declared symbols and skeletons | Exposes an API surface without reading every implementation body; source spans distinguish retained text from generated ellipses. This is directly useful for locating extension points and understanding unfamiliar modules. | Lossy compression can remove exactly the condition, error path, mutation or initializer needed to fix a bug. The model may still need source reads. Symbol/declaration tools already exposed by a client overlap with this capability. |
| Semantic file search | Can rank a file despite a query not naming its exact identifier. A model receives ranked candidates without designing several lexical searches. | The semantic model can rank semantically similar but irrelevant code. An adaptive agent's identifier extraction and follow-up searches are absent from the current baseline comparison. |
| Automatic context assembly | Packages explicit files, semantic anchors, dependency neighborhoods, full source and skeletons into a bounded response. It reduces orchestration work for the model. | The policy also makes decisions on the model's behalf. PageRank importance is not task relevance, and dependency expansion can consume the budget with shared utilities while displacing the needed implementation. |
| Shared fresh generations | Multiple derived views use accepted source buffers; manifest reconciliation repairs missed watcher hints. This is valuable when the working tree changes during a task. | Freshness protects consistency; it does not establish analysis completeness or task correctness. Requests spanning different generations can still require the model to refresh its earlier understanding. |

The strongest incremental use case is a model with only basic shell/read tools working in an unfamiliar Python or JS/TS repository, particularly when it needs repeated impact/navigation queries. For a known two-line change with a clear path, `rg` and a narrow read can be simpler and cheaper. If a client already exposes equivalent language-server navigation, the remaining case is the unified local session, source representation and context packaging. There is no measurement here establishing superiority over an LSP-equipped workflow.

Implementation evidence: `python_shell/atlas/mcp_server.py:183` exposes the tools; `python_shell/atlas/session.py:288` dispatches them from a reconciled generation; `rust_core/src/call_index.rs:428` onward contains lexical/import lookup. README's support matrix explicitly leaves Rust and Go dependency/call analysis unavailable. Those languages still receive parsing, symbols, skeletons and semantic retrieval.

## Token claims require the full transaction

It is defensible to say that a particular skeleton contains fewer tokens than its corresponding full file, once measured. It is not defensible to turn that compression ratio into the same percentage of total agent token savings or to assume that omitted code was unnecessary.

Net savings depend on avoided search results and raw reads, minus Atlas's tool descriptions/schemas where the client supplies them, call arguments, response metadata, pagination, repeated context, corrective queries and subsequent source reads. Model reasoning, generated output and repeated/cached prompt charging are outside the current retrieval metric. The tool-catalog overhead is client-dependent; this review did not measure it.

The implementation already includes important response costs: context rendering counts paths, map text, generation headers, omissions, JSON descriptors and coverage, and checks both text and default JSON formats. JSON includes one source text plus a manifest, rather than duplicating each chunk's source in the manifest. However, structured descriptions consume budget that raw snippets would not. Even default text admission is constrained by the stricter JSON representation budget. Returning two typed relationships for one file pair is useful evidence, but may also print that path twice. Paginated list tools repeat generation/coverage/cursor information.

There is also no targeted full-source range MCP tool. Atlas can deliver complete files through context assembly, but a body omitted from a skeleton may require the host's ordinary read tool or another context request. A workflow of search, skeleton, context, then full read can spend more than an initially well-targeted read.

The requested token budget is not a measurement of the actual host model's complete context. The packer uses tiktoken, defaults to the gpt-4/cl100k encoding, and reserves the query plus 1,000 prompt tokens. It does not know the real tool catalog, conversation history or the tokenizer of every model that can consume MCP. The byte/character ceiling is explicit; the numerical token estimate is tokenizer-specific.

Implementation evidence: `context.py:97` chooses the encoder; `context.py:227` builds source plus manifest; `context.py:294` onward applies query/prompt, metadata and transport budgets; `mcp_server.py:30` renders text/JSON; `session.py:327` reserves coverage overhead.

## What the frozen comparison can establish

The design is materially better than counting retrieved filenames alone. It fixes the task set and repository commits before comparison, uses identical eligible files and packing, tests two budgets, separates preparation from warm ranking/packing, includes failed tasks, and validates retained source spans against the actual delivered text. A signature receives no credit for an omitted body. These controls support a statement such as: “At this budget, this ordering/expansion policy delivered more of the annotated source context than this baseline on these tasks.”

The four arms test narrower components:

- **BM25 versus flat embeddings:** lexical versus semantic ranking under the same Atlas representation and packing policy.
- **Flat embeddings versus Atlas without expansion:** the added anchor/PageRank and architecture-ordering policy.
- **Atlas without expansion versus full Atlas:** the marginal effect of dependency-neighborhood expansion.

The BM25 arm receives Atlas's map/full-source/skeleton packer. This is a fair way to isolate ranking, but it does not compare all of Atlas against plain raw reads and does not isolate skeleton compression's benefit. It is also not a model iteratively using `rg`, choosing snippets, revising a query and running tests. Cold preparation is shared by this harness; BM25 intrinsically does not require embeddings. Use the separate preparation fields rather than attributing all shared setup to each algorithm.

The run does **not** establish fewer tool turns, lower total billed tokens, greater code-edit accuracy, more passing bug-fix tests, less human review, or improved final explanations. There is no coding-completion trial. Gold-source byte recall is a useful retrieval proxy, not correctness: a single missing condition can matter more than many retained lines. File precision can also penalize legitimate supporting code outside the annotation. These are reasons to report both metrics and representative failures rather than interpret either as a universal quality score.

Report paired per-task deltas, not only the four means. There are fifty tasks, not four hundred independent observations: two budgets and four methods reuse the same tasks, and some tasks reuse repositories. Show language-specific outcomes because twenty tasks are Rust/Go, where import/call expansion is unavailable. Ten tasks per language support an initial indication, not a broad guarantee. Lower average returned tokens are not automatically better if gold-byte recall also falls; prefer comparisons of useful retained evidence at the same token budget, and inspect the 8k-versus-12k tradeoff.

Implementation evidence: `benchmarks/bench_retrieval.py:43` freezes sampling; `:122` implements file BM25; `:158` validates delivered byte coverage; `:230` summarizes failures and means; `:249` runs identical packing across arms. This review does not use historical `benchmarks/results/` numbers as evidence for the rebuilt implementation. The old skeleton benchmark still contains a Python-only label, so its old narrative should not be used for present language claims.

## Accuracy risks the model must understand

Supported static matches can reduce a particular source of error: confusing unrelated identifiers or methods. The regression suite now exercises those cases. That is evidence of corrected behavior, not a general accuracy estimate. Coverage counts valid indexed files, while callers/callees remain incomplete for dynamic receivers, callbacks, unmodeled inheritance/runtime behavior and unsupported languages. A large indexed-file count does not imply complete call coverage.

Automatic context can miss relevant files before packing, replace a necessary body with a skeleton, over-promote high-centrality neighbors, or omit non-supported source/configuration formats. Absolute-path and provenance metadata consume space without themselves answering the task. No amount of correct pagination or freshness repairs a poor relevance ranking. Tests and inspection of actual implementation remain necessary before a model treats retrieved context as sufficient to change behavior.

## Recommendation for the final claim

Describe Atlas as useful additional tooling with verified navigation/representation behavior and an evaluated context-selection policy. Prefer targeted calls when the task already identifies a file or symbol. Let the frozen results determine whether automatic context deserves to be the default: if BM25 matches or beats it, retain the targeted tools and skeletons while treating the semantic/PageRank/expansion policy as unproven. Do not advertise overall token savings or coding-accuracy gains from this run alone.

A future end-to-end comparison would hold model, tasks, client prompts and tool budgets constant, compare ordinary search/read against Atlas-enabled access, log all tool/schema/response/follow-up costs, and score completed changes with held-out tests and review. That is a different experiment, not work performed or authorized by this review.
