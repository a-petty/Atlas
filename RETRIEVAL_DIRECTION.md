# Atlas: next direction for discussion

Status: proposed direction, September 5, 2026. This is a discussion note, not an approved implementation scope. No Atlas source changes or new benchmarks were made for this discussion.

A meaningful advantage over BM25 is plausible, but the completed experiment does not demonstrate a reliable overall advantage or justify a promised improvement. The strongest hypothesis is lexical plus semantic retrieval over implementation chunks, with focused structural follow-up and precise source delivery.

The existing comparison used the same skeleton-derived per-file text and the same packer for all four methods. A future comparison must give BM25 the same richer body chunks, eligible source set, and delivery budget as the proposed method. Otherwise representation improvements could be incorrectly attributed to a superior ranking algorithm.

Evidence motivating the direction:

- Atlas's 12k known-byte recall was 9.92%, versus 8.04% for BM25, with broad repository-cluster uncertainty. These are annotated-source coverage proxies, not coding success rates.
- Python results favored BM25 strongly. An enhanced approach should retain lexical retrieval as a candidate source; combining candidates does not itself guarantee preserved final quality under a fixed budget.
- One Django example retrieved the annotated file yet delivered none of its annotated source bytes. File selection and evidence delivery need separate evaluation.
- Graph expansion had no quality effect on any task at 8k and almost none at 12k. The saved results generally cannot separate poor anchors, irrelevant expansion, and packaging losses.
- The embedding representation prefers file skeletons and ranks whole files using their best matching chunk. Implementation content can therefore be absent from the search representation even when it is available in the accepted source snapshot.

Recommended next experiment, if implementation is subsequently requested: first trace a small, varied set of observed wins and losses through source eligibility, candidate ranking, and retained source spans. Use diagnostic oracle file/chunk selection to isolate packaging losses. Then test the smallest change addressing the dominant observed loss: function/method body chunks with paths and enclosing context, lexical and semantic candidate fusion, and focused snippet delivery. Keep graph additions query-specific and measure their separate contribution. Defer a larger embedding model or learned reranker until the earlier stages are understood.

Use the current 50 tasks for diagnosis and development. Evaluate the selected approach on fresh repository-held-out tasks, with a strong chunk-level BM25 baseline and matched delivered-token budgets. Count cold preparation and warm operation costs separately. A later agent experiment should measure correct task completion, total session tokens, and latency, including follow-up reads. Retrieval improvement alone does not establish agent benefit.

External evidence supports the research direction but does not predict Atlas's gains. [RepoCoder](https://aclanthology.org/2023.emnlp-main.151/) reports gains from iterative retrieval and generation for repository code completion. [CodeRAG-Bench](https://aclanthology.org/2025.findings-naacl.176/) finds both useful-context retrieval and generators' use of retrieved material remain bottlenecks.

Open product question: is the first target Alec's agents repeatedly working in a stable set of repositories, or general use on unfamiliar repositories? This affects setup-cost tolerance and which workflows should define success.

## Follow-up: provisional focus on recurring repository work

Alec's response was that he is unsure which path has more value, but probably prefers his own agents revisiting his repositories. Treat this as a working product direction, not a binding decision or authorization to begin implementation.

Recommended initial objective: help Alec's agents make correct maintenance changes in repositories they revisit, using fewer total session tokens. Reuse of an index can amortize preparation costs, but does not itself reduce model input tokens; BM25 can also reuse its index. Measure those benefits separately.

Start by improving retrieval and delivery of current implementation bodies, retaining lexical retrieval and testing focused structural follow-up. Atlas itself is a practical first development fixture. Python maintenance tasks should receive particular attention because the completed comparison showed a substantial Python weakness. Tasks used to develop the approach cannot also establish its generalization performance.

A later experiment could preserve a small task working set across sessions: source references and unresolved questions, revalidated against current code before reuse. Reuse must account for changed, renamed, or deleted source and affected relationships. Persisted summaries must not be treated as current evidence without validation. This remains a separate, unbuilt hypothesis; avoid making it a prerequisite for the first retrieval improvement.

The strongest practical comparison is the same agent, model, task starting state, and ordinary tools, with and without Atlas; include a strong persistent BM25 baseline. Record independently verified task correctness, all session tokens including follow-up reads and retries, and elapsed time. A small personal-repository pilot is directional evidence, not proof of general superiority over BM25.
