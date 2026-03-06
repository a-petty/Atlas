# Atlas

A local-first semantic code intelligence engine that builds symbol-aware dependency graphs of repositories and provides optimized context to LLMs and AI coding tools.

Atlas combines a high-performance **Rust core** (Tree-sitter parsing, graph algorithms, incremental updates) with a **Python orchestration layer** (context assembly, LLM interaction, MCP server) to understand codebases at both the file and sub-file level. It powers smarter AI-assisted development by giving models the right code context automatically.

## Key Capabilities

- **Dependency graph with PageRank** — Builds a weighted directed graph of file-level imports and symbol usages. PageRank scoring surfaces the most architecturally central files in any repository.

- **Code Property Graph (CPG)** — Optional sub-file analysis layer with control flow graphs, reaching definitions dataflow analysis, and cross-file call graphs for Python codebases.

- **Anchor & Expand context assembly** — Semantic vector search finds query-relevant files (anchor), then graph traversal pulls in their dependencies (expand). Three-tier token budgeting adapts to both repository size and model context window.

- **MCP server** — Exposes 12 tools over the [Model Context Protocol](https://modelcontextprotocol.io/) for integration with Claude Code, VS Code, and other MCP-compatible clients.

- **Multi-language support** — Tree-sitter parsing for Python, Rust, JavaScript (+ JSX), TypeScript (+ TSX), and Go. Import resolution for Python and JS/TS. CPG analysis for Python.

- **Incremental updates** — 4-tier change classification (Local/FileScope/GraphScope/FullRebuild) avoids unnecessary recomputation when files change. File watching keeps the graph in sync as you edit.

- **Skeleton generation** — Produces compressed representations of files (signatures + docstrings, bodies replaced with `...`) that preserve the full API surface at 70-90% token savings.

## Architecture

```
                        ┌─────────────────────────────────────────────┐
                        │         MCP Server (mcp_server.py)          │
                        │  12 tools: status, map, deps, callgraph,   │
                        │  semantic search, context assembly, ...     │
                        └────────────────────┬────────────────────────┘
                                             │
┌────────────────────────────────────────────┼────────────────────────────────────┐
│  Python Shell (python_shell/atlas/)        │                                    │
│                                            │                                    │
│  ┌───────────────┐  ┌───────────────────┐  │    ┌────────────┐  ┌────────────┐  │
│  │    Agent      │  │ Context Manager   │  │    │  Embedding │  │    LLM     │  │
│  │ (orchestrator,│  │ (anchor+expand,   │◄─┘    │  Manager   │  │  Clients   │  │
│  │  watch loop,  │  │  3-tier budget,   │       │ (FastEmbed,│  │ (Ollama,   │  │
│  │  tool exec)   │  │  adaptive params) │       │  cosine    │  │  MLX,      │  │
│  └───────┬───────┘  └────────┬──────────┘       │  similarity│  │  stub)     │  │
│          │                   │                  └────────────┘  └────────────┘  │
│          │                   │           PyO3 boundary                          │
├──────────┼───────────────────┼──────────────────────────────────────────────────┤
│  Rust Core (rust_core/src/)  │                                                  │
│                              │                                                  │
│  ┌───────────────────────────┼──────────────────────────────────────────────┐   │
│  │  RepoGraph (graph.rs)     │                                              │   │
│  │  ┌───────────────┐  ┌─────┴─────────┐  ┌──────────────┐  ┌────────────┐  │   │
│  │  │  File-Level   │  │   PageRank    │  │   Symbol     │  │  Import    │  │   │
│  │  │  DiGraph      │  │  (weighted,   │  │   Index      │  │  Resolver  │  │   │
│  │  │ FileNode ──── │  │   iterative)  │  │ name→files   │  │ (Python,   │  │   │
│  │  │  ──► EdgeKind │  │               │  │ file→symbols │  │  JS/TS)    │  │   │
│  │  └───────────────┘  └───────────────┘  └──────────────┘  └────────────┘  │   │
│  └──────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
│  ┌──────────────────────────────────────────────────────────────────────────┐   │
│  │  CpgLayer (cpg.rs) — Optional, Python-only                               │   │
│  │  ┌───────────┐  ┌───────────┐  ┌───────────────┐  ┌─────────────────┐    │   │
│  │  │   CFG     │  │ Dataflow  │  │  Call Graph   │  │  AST Nodes      │    │   │
│  │  │ (cfg.rs)  │  │(dataflow  │  │ (callgraph    │  │ (functions,     │    │   │
│  │  │ if/for/   │  │  .rs)     │  │  .rs)         │  │  classes,       │    │   │
│  │  │ while/try │  │ reaching  │  │ 2-pass:       │  │  methods,       │    │   │
│  │  │ break/    │  │ defs,     │  │ extract →     │  │  variables,     │    │   │
│  │  │ continue/ │  │ worklist  │  │ resolve       │  │  statements)    │    │   │
│  │  │ return    │  │ algorithm │  │ cross-file    │  │                 │    │   │
│  │  └───────────┘  └───────────┘  └───────────────┘  └─────────────────┘    │   │
│  └──────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
│  ┌────────────────────────────────┐  ┌───────────────────────────────────────┐  │
│  │  Parser (parser.rs)            │  │  Watcher (watcher.rs)                 │  │
│  │  Tree-sitter: 5 languages      │  │  FSEvents (macOS) + notify crate      │  │
│  │  SymbolHarvester + queries/    │  │  100ms debouncing, .gitignore-aware   │  │
│  │  Skeleton generation           │  │  crossbeam-channel to main thread     │  │
│  │  Syntax checking               │  │                                       │  │
│  └────────────────────────────────┘  └───────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### Dual-Graph Model

Atlas maintains two graph layers with different granularity:

**File-Level Graph** (`graph.rs` → `RepoGraph`): A `DiGraph<FileNode, EdgeKind>` where nodes are source files and edges are `Import` (structurally confirmed via AST) or `SymbolUsage` (name-matched, import-gated) relationships. Parallel parsing with rayon, weighted PageRank where Import edges carry 2x weight over SymbolUsage edges (structurally confirmed dependencies rank higher than heuristic name matches).

**CPG Overlay** (`cpg.rs` → `CpgLayer`): Fine-grained `DiGraph<CpgNode, CpgEdge>` operating at sub-file granularity. Nodes are functions, methods, classes, variables, statements, and CFG sentinels. Edges include control flow (if/else, loops, exceptions), reaching definitions dataflow, and cross-file call/calledBy relationships with argument and return value flow. Built in four phases:

```
build_file(path):                        Per-file (parallelizable)
  Phase 1: Extract AST nodes (functions, classes, variables)
  Phase 2: Build intra-procedural CFG per function
  Phase 3: Reaching definitions analysis per function
  Phase 4a: Extract call sites per function

resolve_all() / resolve_file():          Cross-file (after all files built)
  Phase 4b: Resolve call sites → Calls/CalledBy/DataFlowArgument/DataFlowReturn edges
```

### Incremental Updates

When a file changes, Atlas classifies the change by comparing content hashes and applies the minimal update:

| Tier | Trigger | Action |
|------|---------|--------|
| **Local** | Only function bodies changed (imports, definitions, usages unchanged) | Update content hash, rebuild CPG for file. No edge changes. PageRank stays valid. |
| **FileScope** | Definitions or usages hash changed (function added/removed/renamed) | Re-harvest symbols, update symbol index, rebuild SymbolUsage edges. Flag PageRank dirty. |
| **GraphScope** | Imports hash changed | Everything in FileScope + re-resolve import edges. Previously unresolved imports may now resolve. |

### Context Assembly: Anchor & Expand

When a user asks a question, `ContextManager` assembles an optimized prompt using adaptive three-tier token budgeting. All parameters scale dynamically based on the target model's context window, repository size, and graph density.

**Tier 1 — Repository Map** (measured cost, capped at 8% of budget): PageRank-ranked architectural overview with directory structure. Gives the model spatial awareness of the project.

**Tier 2 — Full File Content** (40-75% of remaining budget, inversely proportional to repo size):
1. *Explicit files* — caller-specified files of interest (highest priority)
2. *Anchor files* — semantic vector search finds the most query-relevant files via cosine similarity of FastEmbed embeddings
3. *Neighborhood expansion* — multi-hop BFS walks the dependency graph from anchors, with edge-type-aware traversal (SymbolUsage edges get full hop depth, Import edges get 1 hop to prevent transitive explosion) and distance decay weighting (hop n → weight 1/2^(n-1))

**Tier 3 — Architectural Skeletons** (remaining budget): Signatures and docstrings of high-PageRank files, with function bodies replaced by `...`. Provides panoramic awareness of the project's most important interfaces without consuming implementation-detail tokens. Three-level sourcing: dependency neighbors of anchors → additional semantic search → PageRank fallback.

## Quick Start

### Prerequisites

- Python >= 3.10
- Rust toolchain ([rustup](https://rustup.rs/))

### Installation

```bash
git clone https://github.com/a-petty/Atlas.git
cd Atlas

# Create and activate a virtual environment
python -m venv .venv
source .venv/bin/activate

# Install dependencies and build the Rust core (one command)
pip install -e ".[mcp]" && maturin develop
```

This installs all Python dependencies (including MCP server support) and compiles the Rust core as a Python extension. After any change to Rust code, re-run `maturin develop`.

### Optional Dependencies

```bash
# Development tools (pytest, mypy)
pip install -e ".[dev]"

# Apple Silicon local LLM inference via MLX
pip install -e ".[mlx]"
```

## Usage

### MCP Server (Claude Code, VS Code, etc.)

The primary way to use Atlas is as an MCP server that provides semantic code intelligence to AI coding tools.

```bash
# Start the MCP server (defaults to current directory if --project-root is omitted)
atlas-mcp --project-root /path/to/repo

# With debug logging
atlas-mcp --project-root /path/to/repo --verbose
```

#### Claude Code configuration

**Global** (`~/.claude/settings.json`) — use `--project-root` to target a specific repo:
```json
{
  "mcpServers": {
    "atlas": {
      "command": "/path/to/Atlas/.venv/bin/atlas-mcp",
      "args": ["--project-root", "/path/to/your/repo"]
    }
  }
}
```

**Per-project** (`.claude/settings.json` in the repo you want to analyze) — no `--project-root` needed since Claude Code sets the working directory to the project root:
```json
{
  "mcpServers": {
    "atlas": {
      "command": "/path/to/Atlas/.venv/bin/atlas-mcp"
    }
  }
}
```

#### Cursor configuration

**`.cursor/mcp.json`**:
```json
{
  "mcpServers": {
    "atlas": {
      "command": "/path/to/Atlas/.venv/bin/atlas-mcp",
      "args": ["--project-root", "/path/to/your/repo"]
    }
  }
}
```

#### VS Code configuration

**`.vscode/mcp.json`**:
```json
{
  "servers": {
    "atlas": {
      "command": "/path/to/Atlas/.venv/bin/atlas-mcp",
      "args": ["--project-root", "${workspaceFolder}"]
    }
  }
}
```

The MCP server uses lazy initialization — the graph is built on first tool call, CPG is enabled on first CPG tool call, and embeddings are loaded on first semantic search.

### MCP Tools

Atlas exposes 12 tools over MCP:

| Tool | Description |
|------|-------------|
| `atlas_status` | Graph statistics and readiness status. Call first to verify Atlas is ready. |
| `get_repository_map` | PageRank-ordered architecture overview with directory structure and importance scores. |
| `get_dependencies` | Outgoing dependencies for a file (what this file imports/uses). |
| `get_dependents` | Incoming dependents for a file (blast radius — what depends on this file). |
| `get_top_ranked_files` | Most architecturally important files ranked by PageRank score. |
| `find_relevant_files` | Semantic search via vector embeddings — finds files related to a natural language query. |
| `assemble_context` | Core intelligence: Anchor & Expand context assembly with three-tier token budgeting. |
| `get_file_symbols` | Functions, methods, and classes in a file with signatures, docstrings, and line numbers. Python only. |
| `get_callees` | Outgoing call graph — what functions does this function call? Python only. |
| `get_callers` | Incoming call graph — what functions call this function? Python only. |
| `get_file_skeleton` | Function/class signatures without implementation bodies. All supported languages. |
| `atlas_refresh` | Re-scan repository and rebuild graph from scratch (after branch switches, large merges, etc.). |

### Standalone CLI

Atlas also includes a standalone CLI for direct interaction with local LLMs:

```bash
# Watch a repository — builds the graph and keeps it updated
atlas watch /path/to/repo

# One-shot query against a repository
atlas query "Explain the authentication flow" -p /path/to/repo

# Interactive chat session with tool use
atlas chat -p /path/to/repo

# Specify a different Ollama model
atlas query "Find all unused imports" -p /path/to/repo --model codellama

# Use MLX on Apple Silicon
atlas chat -p /path/to/repo --provider mlx
```

The CLI requires [Ollama](https://ollama.ai/) (default) or MLX for LLM inference.

## Configuration

### `.atlas.toml` (optional)

Place at the project root to specify explicit source roots for import resolution:

```toml
[project]
source_roots = ["src", "lib", "packages"]
```

Without this, Atlas auto-detects source roots by looking for directories containing `__init__.py` files or namespace packages.

### `.atlasignore` (optional)

Place at the project root to exclude additional directories from scanning. One pattern per line; `#` for comments:

```
# Exclude vendored code
vendor/
third_party/

# Exclude generated files
generated/
```

**Default ignored directories**: `node_modules`, `target`, `.git`, `__pycache__`, `dist`, `build`, `.venv`, `venv`

Atlas also respects `.gitignore` files during repository scanning. See [Troubleshooting](#troubleshooting) if this causes issues.

## Troubleshooting

### High unresolved import count / missing files

Atlas uses `.gitignore` rules during file scanning. If your `.gitignore` contains broad directory patterns (e.g., `models/` to ignore ML model binaries), those patterns may also exclude source code directories that happen to share the same name.

**Symptoms:**
- `atlas_status` reports a high percentage of unresolved imports
- Files that exist on disk don't appear in the dependency graph
- Import edges are missing for an entire package

**Diagnosis:** Check whether your source files are being excluded by `.gitignore`:

```bash
# Check if a specific file is ignored
git check-ignore -v path/to/suspected/file.py

# Compare files on disk vs files Atlas can see
ls path/to/package/*.py
git ls-files path/to/package/
```

**Common cause:** A `.gitignore` rule like `models/` (intended for ML binary artifacts) matching a Python package like `app/models/`. If those files were never `git add -f`'d, they'll be invisible to both git and Atlas — even though they exist on disk and Python imports them fine.

**Fixes:**
1. Make the `.gitignore` pattern more specific (e.g., `*.bin`, `*.gguf`, `*.onnx` instead of `models/`)
2. Force-add the source files: `git add -f path/to/package/*.py`
3. Or add the directory as a negation in `.gitignore`: `!app/models/`

## How It Works — A Concrete Example

Scenario: You have a Django project with ~200 Python files. You ask Atlas-powered Claude Code: *"Add a new API endpoint POST /api/v2/teams/ that creates a team and assigns the authenticated user as owner."*

**1. Graph construction** (already done at startup): Atlas parsed all 200 files in parallel, harvested ~1,500 symbols, resolved ~800 import edges and ~600 symbol usage edges. PageRank identified `models/user.py`, `core/auth.py`, and `utils/db.py` as the most central files.

**2. Anchor**: Embedding search finds the 10 most query-relevant files: `views/teams.py`, `models/team.py`, `serializers/team.py`, `urls/api_v2.py`, etc.

**3. Expand**: BFS walks the dependency graph from anchors — `views/teams.py` imports `core/auth.py` and `serializers/team.py` (hop 1), which import `models/user.py` and `serializers/base.py` (hop 2). Import-only edges stop at 1 hop to prevent transitive explosion.

**4. Context assembly**: Tier 1 gets the repo map (~2K tokens). Tier 2 gets full content of ~12 key files (~46K tokens). Tier 3 gets skeletons of ~60 high-PageRank files (~28K tokens) — the model can see that `utils/permissions.py` has `def check_team_permission(user, team, action)` without its 50-line body.

**5. Result**: The model receives a prompt with the exact files it needs — existing team model schema, auth decorators, base serializer class, URL routing patterns, and the signatures of permission utilities. It generates code that matches the project's actual conventions.

Without Atlas, you'd manually paste 3-4 files and likely miss the base serializer class or auth decorator pattern. Atlas automates context selection entirely — the developer just asks the question.

## Development

```bash
# Activate virtual environment
source .venv/bin/activate

# Build Rust core and install as Python extension (run from project root)
maturin develop

# Run all Rust tests
cd rust_core && cargo test

# Run a single Rust test file
cargo test --test test_callgraph

# Run a single Rust test by name
cargo test --test test_callgraph test_resolve_same_file

# Run all Python tests
pytest python_shell/tests/

# Run a single Python test file
pytest python_shell/tests/test_context.py

# Run Rust benchmarks
cd rust_core && cargo bench
```

### Test Suites

**Rust** (`rust_core/tests/`): 14 test files covering graph construction, incremental updates and parsing, CPG node extraction, CFG construction, reaching definitions dataflow, call graph extraction and resolution, symbol harvesting, import canonicalization, JS/TS import resolution, PageRank, repo map generation, and file watching.

**Python** (`python_shell/tests/`): 8 test files covering context assembly (multi-hop BFS, adaptive parameters, token budgeting, model context windows), graph updates, embeddings, chat/agent loop, LLM client abstraction, parser integration, response parsing, tool execution with syntax checking, and error handling.

### Project Structure

```
Atlas/
├── rust_core/
│   ├── src/
│   │   ├── graph.rs            # RepoGraph: file-level graph, PageRank, incremental updates
│   │   ├── cpg.rs              # CpgLayer: sub-file graph (functions, CFG, dataflow, calls)
│   │   ├── cfg.rs              # CfgBuilder: per-function control flow graph construction
│   │   ├── dataflow.rs         # DataFlowAnalyzer: worklist-based reaching definitions
│   │   ├── callgraph.rs        # CallGraphBuilder: two-pass call graph (extract → resolve)
│   │   ├── parser.rs           # Tree-sitter parsing, SymbolHarvester, skeleton generation
│   │   ├── symbol_table.rs     # SymbolIndex: name→files and file→symbols mappings
│   │   ├── import_resolver.rs  # Python and JS/TS import resolution
│   │   ├── watcher.rs          # File system monitoring via notify crate
│   │   └── lib.rs              # PyO3 bindings exposing RepoGraph to Python
│   ├── queries/                # Tree-sitter S-expression queries per language
│   │   ├── python/
│   │   ├── rust/
│   │   ├── javascript/
│   │   └── typescript/
│   └── tests/                  # 14 integration test files
├── python_shell/
│   └── atlas/
│       ├── mcp_server.py       # MCP server: 12 tools, lazy init, stdio transport
│       ├── context.py          # ContextManager: Anchor & Expand, 3-tier budgeting
│       ├── agent.py            # AtlasAgent: orchestrator, watch loop, tool execution
│       ├── cli.py              # CLI entry point: watch, query, chat subcommands
│       ├── tools.py            # ToolExecutor: file ops with syntax validation
│       ├── embeddings.py       # EmbeddingManager: FastEmbed vector search
│       └── llm.py              # LLM clients: Ollama, MLX, Stub
├── pyproject.toml              # Python package config, entry points, optional deps
├── rust_core/Cargo.toml        # Rust dependencies and test declarations
└── docs/                       # Architecture docs, evaluation reports, roadmaps
```

### Key Design Decisions

- **All parsing goes through Tree-sitter** via the Rust core. Source code is never parsed in Python.
- **PyO3 boundary**: Rust types have `Py*` wrapper structs in `lib.rs` (e.g., `PyRepoGraph` wraps `RepoGraph`). Errors convert via `From<GraphError> for PyErr`.
- **Stateless analyzers**: `CfgBuilder`, `DataFlowAnalyzer`, and `CallGraphBuilder` are stateless structs with associated functions that mutate `CpgLayer` in-place.
- **Conservative call resolution**: The call graph only creates edges for unambiguously resolved calls. Ambiguous or builtin calls are left unresolved rather than creating false-positive edges.
- **Swap-remove safety**: `petgraph::DiGraph::remove_node` uses swap-remove, so all side-maps must be remapped when a node is removed. See `cpg.rs::remove_file()` for the pattern.
- **Parallel-then-serial**: File parsing is parallelized via rayon; graph mutation is serialized (petgraph requires exclusive access).
- **Reflexive Sensory Loop**: When the agent writes a file, `ToolExecutor` validates syntax via the Rust core's `check_syntax()` before saving. Invalid writes are refused and the error is returned to the LLM for correction.

## Rust Core Dependencies

| Crate | Purpose |
|-------|---------|
| `tree-sitter` + language grammars | Parsing for Python, Rust, JS/JSX, TS/TSX, Go |
| `petgraph` | Directed graph implementation for file-level and CPG graphs |
| `pyo3` | Python ↔ Rust FFI bindings |
| `rayon` | Parallel file parsing |
| `notify` + `notify-debouncer-full` | File system monitoring with 100ms debouncing |
| `ignore` | .gitignore-aware file walking |
| `parking_lot` | Efficient mutex/sync primitives |
| `crossbeam-channel` | Thread-safe message passing for watcher events |
| `lru` | Skeleton cache (500 entries) |
| `thiserror` | Error type derivation |

## Python Dependencies

| Package | Purpose |
|---------|---------|
| `tiktoken` | Token counting for context budget management |
| `fastembed` | Vector embeddings for semantic search (BAAI/bge-small-en-v1.5) |
| `rich` | Terminal formatting and progress display |
| `pydantic` | Configuration validation |
| `ollama` | Local LLM inference client |
| `numpy` | Embedding vector operations |
| `mcp[cli]` | MCP server framework (optional) |
| `mlx-lm` | Apple Silicon LLM inference (optional) |

## License

This project is licensed under the [MIT License](LICENSE).
