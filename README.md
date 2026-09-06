# Atlas

Atlas is a local code intelligence engine for repository maps, dependency and call analysis, semantic search, and source-attributed context. Rust parses source and builds indexes; Python manages repository sessions, retrieval and MCP/CLI interfaces. Parsing, embedding inference and cache storage run locally.

## Supported analysis

| Capability | Python / `.pyi` | JS / JSX / MJS / CJS | TS / TSX | Rust | Go |
|---|---|---|---|---|---|
| Parsing, declared symbols, API skeletons | Yes | Yes | Yes | Yes | Yes |
| Import and symbol-use dependency evidence | Yes | Yes | Yes | Unavailable | Unavailable |
| Static lexical/import call analysis | Yes | Yes | Yes | Unavailable | Unavailable |
| Optional CFG/data-flow overlay | Yes | Existing JS support | Existing JS-compatible support | Unavailable | Unavailable |

Call analysis follows lexical bindings, imported aliases, module members, statically established class instances and supported reexports. Dynamic receivers, ambiguous imports and shadowed names remain unresolved. A duplicate simple function name produces qualified alternatives rather than selecting a method arbitrarily. This is static analysis, not proof of all possible runtime calls.

Skeletons preserve declarations, documentation, decorators/attributes, fields, imports, constants and types while omitting executable bodies and large initializers. Compression depends on the file's API-to-implementation ratio; there is no universal savings percentage. JSON records actual retained UTF-8 byte ranges separately from generated placeholders.

## Install and verify

Use Python 3.11 or later, a Rust toolchain, and `uv`. The package metadata permits Python 3.10; the verification matrix uses 3.11.

```sh
uv sync --locked --python 3.11 --extra dev --extra mcp --extra benchmark
uv run --no-sync python scripts/verify.py --release
```

The verifier checks the Python lock, runs Rust tests, rebuilds the current extension, then runs Python and actual stdio MCP tests. It does not accept an old installed extension as verification of new Rust source. CI defines Linux and macOS checks with the locally verified uv 0.11.26 and Rust 1.93.0 versions; local verification is recorded separately from CI execution.

After a Rust-only edit, a quick rebuild is:

```sh
uv run --no-sync maturin develop --release
```

`uv.lock` and `rust_core/Cargo.lock` capture dependencies. The first semantic query may download the FastEmbed BGE model if it is absent. Graph, call, symbol and skeleton tools do not require the model. `HF_HUB_OFFLINE=1` can enforce an offline run after the model has been cached.

## Repository sessions

MCP and the CLI share the `RepositorySession` implementation. Each session owns a worker process with its graph, captured source buffers, AST call index, embedding matrix and optional detailed overlays. Requests are serialized within that session.

Before each answer, Atlas reconciles file and configuration metadata. Native watcher events are hints; missed events do not prevent the next request from seeing edits. Changed buffers are read without newline conversion. Atlas checks the manifest again before accepting a generation, and answers from those accepted buffers. A later edit belongs to the next generation.

An ordinary content edit updates the affected indexes. Create/delete/rename operations and relevant resolver, dependency, ignore or source-root configuration changes trigger a fresh graph epoch. A syntax-invalid file is excluded from graph/retrieval coverage until repaired; other files remain usable. Status and every JSON page expose incomplete coverage.

A timeout or active cancellation terminates and joins the worker. The next request starts a clean generation. Cancelling a queued request cannot kill another caller's work. Finished embedding batches survive in the content cache, so a cold preparation can resume after restart. For large repositories, set `ATLAS_REQUEST_TIMEOUT` in the server environment to cover expected cold preparation and configure the MCP client deadline accordingly. The default is 120 seconds; some measured snapshots required more than 20 minutes, so even a 900-second override would not cover every first preparation. Status can report preparation progress while the query runs. Incomplete preparation never returns a complete-looking semantic ranking.

The eager AST call index is independent of CFG/data-flow materialization. Detailed CPG files use a 128-file LRU by default; Rust callers can set another positive file capacity. Eviction does not change the public lightweight caller/callee answers. The diagnostic full-CPG command visits files but retains only the bounded overlay cache.

## MCP

```sh
uv run --no-sync atlas-mcp --project-root /path/to/repository
```

Configure an MCP client to invoke the environment's `atlas-mcp` executable with `--project-root`. Atlas exposes twelve regular tools and one explicit diagnostic:

| Tool | Purpose |
|---|---|
| `atlas_status` | Generation, coverage, index statistics and preparation progress |
| `get_repository_map` | PageRank-ordered architecture overview |
| `get_dependencies`, `get_dependents` | Typed file relationships |
| `get_top_ranked_files` | Architectural importance |
| `find_relevant_files` | Semantic file ranking |
| `assemble_context` | Budgeted map, full source and skeleton chunks |
| `get_file_symbols`, `get_file_skeleton` | Declared symbols and API representations |
| `get_callers`, `get_callees` | Static function relationships |
| `atlas_refresh` | Explicit configuration/graph rebuild |
| `diag_full_cpg_build` | Detailed-overlay diagnostic inside the worker timeout boundary |

Text remains the default. Set `response_format="json"` for structured results. List tools return a bounded page and an opaque `next_cursor`; continue with the same tool/query and cursor. A changed generation rejects an old cursor instead of mixing revisions. Page limits range from 1 to 1,000. A single declaration too large for a page is reported explicitly; context assembly can omit it with a reason.

`assemble_context` accepts `query`, `files_in_scope`, `max_tokens` (default 12,000), `include_map`, and `response_format`. It reserves the query plus 1,000 prompt tokens, includes a map capped at 8%, and divides remaining source space between full files and skeletons. Explicit files lead, followed by semantic anchors, dependency neighborhoods and remaining ranked files. A full file that does not fit can fall back to whole skeleton declarations without losing its priority.

Both default JSON serialization and plain text fit the 60,000-character transport limit and the available token budget. Coverage and manifest overhead count. Omission detail is added only after source admission; diagnostic paths cannot displace a selected source chunk. Empty-budget results are status metadata with no source. There is no final character slicing.

Each JSON context contains `generation`, `text`, `chunks`, `omissions`, `counts`, and `coverage`. Chunks identify file, representation, selection reason, source spans and character offsets in the delivered text. Byte spans are half-open UTF-8 offsets; physical lines are 1-based and inclusive. Generated ellipses receive no source coverage credit.

## Configuration and caches

An optional `.atlas.toml` controls resolver selection and source roots:

```toml
[project]
languages = ["python", "typescript"]
source_roots = ["src", "packages/shared/src"]
```

Omit `languages` to detect resolver groups automatically. Explicit selection still reports all observed languages and unavailable analysis. Repository `.gitignore` rules are respected; tracked files remain visible as in Git. Standard generated directories are excluded, including `.git`, `.venv`, `node_modules`, `target`, `build` and `dist`. `.atlasignore` supports relative glob patterns, directory names and later `!` exceptions within the scanned tree. It does not re-include a directory excluded by the standard scanner.

Embeddings are keyed by embedding input, model content/configuration, engine version and chunk policy. The live matrix uses normalized chunk vectors and max-chunk cosine similarity. All query and source chunks respect the actual model tokenizer limit. Atomic persistent batches allow restart reuse, while content edits and path-prefix changes invalidate the corresponding input.

| Setting | Default |
|---|---|
| `ATLAS_CACHE_DIR` | `~/.cache/atlas` (outside the checkout) |
| `ATLAS_EMBEDDING_CACHE_BYTES` | 2 GiB; oldest-accessed eviction; `0` disables persistence |
| `ATLAS_DIAGNOSTICS` | Unset; opt in to detailed Rust diagnostics |
| `ATLAS_REQUEST_TIMEOUT` | 120 seconds; positive finite worker deadline shared by MCP and CLI |

The CLI still supports its existing stub, Ollama and optional MLX clients. Model providers do not own Atlas's repository state. This implementation adds no hosted agent runtime or paid model trial.

## Evaluation

`benchmarks/manifests/contextbench_50_v1.json` freezes fifty public ContextBench tasks: ten each for Python, JavaScript, TypeScript, Rust and Go, excluding the ten historical development examples. Dataset hash, revision, task IDs, repository URLs, full commits and seed are recorded before comparison.

```sh
uv run --no-sync python -m benchmarks.bench_retrieval run \
  --dataset /path/to/pinned/contextbench.parquet \
  --repo-cache /path/to/local/checkouts \
  --output /path/to/results.json
```

The runner compares BM25, flat embeddings, Atlas without expansion, and Atlas with expansion at 8k and 12k budgets. All use identical eligible files, representation/packing rules, prompt allowance and transport ceilings. Metrics credit only delivered retained source bytes; failures remain in the denominator. Cold graph/model preparation and warm retrieval latency are separate. `--resume` validates dataset, manifest, model, dependency and implementation fingerprints; partial/failed tasks rerun as a unit.

`benchmarks/bench_scale.py` constructs a deterministic 10,000-file mixed-language fixture and measures actual worker requests, preparation, warm p95, edit-to-answer latency and peak RSS.

The September 5, 2026 evaluation passed the agreed warm gates on the 10,000-file fixture and pinned Django. Verification includes 313 Rust tests and 209 Python tests. The complete frozen comparison found no demonstrated retrieval advantage for graph expansion over the simpler embedding variants; total agent-token savings and coding-task accuracy were not measured. See the [implementation report](IMPLEMENTATION_REPORT.md) and [reproduction evidence](reports/feature-parity-2026-09-05/REPRODUCE.md) for results, cold preparation costs, grader correction and limitations.
