# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Atlas is a local-first autonomous coding agent that combines a high-performance Rust core with Python orchestration. It builds symbol-aware semantic graphs of repositories and provides intelligent context to LLMs for code modifications.

## Build & Development

The project uses **Maturin** to bridge Rust and Python. The Rust core compiles into a Python extension module (`atlas.semantic_engine`).

```bash
# Activate virtual environment first
source .venv/bin/activate

# Build Rust core and install as Python extension (run from project root)
maturin develop

# Run Rust tests
cd rust_core && cargo test

# Run a single Rust test file
cargo test --test test_callgraph

# Run a single Rust test by name
cargo test --test test_callgraph test_resolve_same_file

# Run Python tests
cd python_shell && pytest

# Run a single Python test
pytest python_shell/tests/test_tools.py

# Run Rust benchmarks
cd rust_core && cargo bench

# Run the CLI
atlas watch /path/to/repo
atlas query "your question" -p /path/to/repo
```

After any change to Rust code, you must run `maturin develop` before Python tests will reflect those changes.

## Architecture

### Dual-Graph Model

Atlas maintains two graph layers with different granularity:

1. **File-Level Graph** (`graph.rs` → `RepoGraph`): `DiGraph<FileNode, EdgeKind>` where nodes are files and edges are `Import` or `SymbolUsage` relationships. Used for PageRank scoring, dependency analysis, and context assembly. Built via parallel parsing with rayon.

2. **CPG Overlay** (`cpg.rs` → `CpgLayer`): Fine-grained `DiGraph<CpgNode, CpgEdge>` with sub-file nodes (functions, methods, classes, statements, CFG sentinels). Enabled via `enable_cpg()`. Python-only initially.

The CPG is built in four phases during `build_file()`, with a cross-file resolution pass after all files:

```
build_file(path):
  Phase 1: Extract CpgNodes (functions, classes, variables) from AST
  Phase 2: Build intra-procedural CFG per function (cfg.rs → CfgBuilder)
  Phase 3: Reaching definitions analysis per function (dataflow.rs → DataFlowAnalyzer)
  Phase 4a: Extract call sites per function (callgraph.rs → CallGraphBuilder)

build_complete() / resolve_all():
  Phase 4b: Resolve call sites across all files → Calls/CalledBy/DataFlowArgument/DataFlowReturn edges
```

### Rust Core (`rust_core/src/`)

- `graph.rs` — `RepoGraph`: file-level graph with PageRank, incremental updates (4-tier change classification: Local/FileScope/GraphScope/FullRebuild), swap-remove-safe node deletion.
- `cpg.rs` — `CpgLayer`: sub-file graph. `CpgNode` kinds: Function, Method, Class, Variable, Statement, CfgEntry, CfgExit. `CpgEdge` kinds: AstChild, ControlFlow*, DataFlowReach, Calls, CalledBy, DataFlowArgument, DataFlowReturn.
- `cfg.rs` — `CfgBuilder`: per-function CFG construction from tree-sitter AST. Handles if/else, for/while, try/except, return/break/continue.
- `dataflow.rs` — `DataFlowAnalyzer`: worklist-based reaching definitions. Extracts defs/uses per statement, computes GEN/KILL/IN/OUT sets, creates DataFlowReach edges.
- `callgraph.rs` — `CallGraphBuilder`: two-pass call graph. Pass 1 extracts `CallSite`s per function. Pass 2 resolves callees (builtins filtered, self.method via parent class, same-file, cross-file via SymbolIndex). Conservative: only resolves unambiguous calls.
- `parser.rs` — Tree-sitter parsing across 6 languages (Python, Rust, JS, TS, Go, Java). `SymbolHarvester` uses tree-sitter queries (`rust_core/queries/`) for symbol extraction. `create_skeleton` strips function bodies.
- `symbol_table.rs` — `SymbolIndex`: maps symbol names → defining file paths, file paths → used symbol names.
- `import_resolver.rs` — Language-specific import resolution (`PythonImportResolver`, `JsTsImportResolver`).
- `watcher.rs` — File system monitoring via `notify` crate with debouncing.
- `lib.rs` — PyO3 bindings. `PyRepoGraph` wraps `RepoGraph`. Custom exceptions: `GraphError`, `ParseError`, `NodeNotFoundError`.

### Python Shell (`python_shell/atlas/`)

- `agent.py` — `AtlasAgent` orchestrator: initializes graph, manages file watching, handles queries, implements the "Reflexive Sensory Loop" (syntax validation before saving files).
- `context.py` — `ContextManager` using "Anchor & Expand" strategy: vector search finds relevant code (anchor), then graph traversal pulls in dependencies (expand). Three-tier token budgeting: Tier 1 (5%) repo map, Tier 2 (50%) full file content, Tier 3 (45%) skeletons of high-PageRank files.
- `llm.py` — LLM clients (Ollama for local models).
- `tools.py` — `ToolExecutor` for file read/write with syntax checking.
- `embeddings.py` — FastEmbed-based vector search.
- `cli.py` — Entry point (`atlas` command). Subcommands: `watch`, `query`, `chat`.

### Key Design Patterns

- **All parsing goes through Tree-sitter** via the Rust core — never parse source code in Python.
- **PyO3 boundary**: Rust types have `Py*` wrapper structs in `lib.rs` (e.g., `PyRepoGraph` wraps `graph::RepoGraph`). Error conversion uses `From<graph::GraphError> for PyErr`.
- **Incremental updates**: File changes are classified into 4 tiers (Local → GraphScope). Only structural changes trigger edge recalculation and PageRank recomputation. CPG updates on every change.
- **Python syntax checking** uses Python's native `compile()` via PyO3 for accuracy; other languages use tree-sitter.
- **Stateless analyzers**: `CfgBuilder`, `DataFlowAnalyzer`, `CallGraphBuilder` are stateless structs with associated functions that mutate `CpgLayer` in-place.
- **Swap-remove safety**: `petgraph::DiGraph::remove_node` uses swap-remove. All side-maps (`file_to_nodes`, `function_to_entry/exit`, `call_sites`, `name_to_funcs`, `stmt_defs/uses`) must be remapped when a node is removed. Check `cpg.rs::remove_file()` for the pattern.

## Rust Test Suites

Tests live in `rust_core/tests/`. Each test file is declared in `Cargo.toml` as a `[[test]]` entry.

| Test file | Covers |
|-----------|--------|
| `test_cpg` | CPG node extraction (functions, classes, variables, decorated, async) |
| `test_cfg` | CFG construction (sequential, if/else, loops, try/except, return/break) |
| `test_dataflow` | Reaching definitions (linear, kill, branch, loop, parameters) |
| `test_callgraph` | Call graph (extraction, resolution, arg/return flow, integration) |
| `test_graph_construction` | File-level graph building |
| `test_graph_updates` | Incremental add/remove/update with consistency validation |
| `test_symbol_harvesting` | Tree-sitter symbol extraction queries |
| `test_watcher` | File system event monitoring |

Common test helper pattern used across CPG/CFG/dataflow/callgraph tests:
```rust
fn build_cpg_for_source(cpg: &mut CpgLayer, path: &str, source: &str) {
    let path = PathBuf::from(path);
    let mut parser = TreeSitterParser::new();
    parser.set_language(SupportedLanguage::Python.get_parser().unwrap()).unwrap();
    let tree = parser.parse(source, None).unwrap();
    cpg.build_file(&path, tree, source.to_string(), SupportedLanguage::Python);
}
```

## Conventions

- Performance-sensitive code belongs in `rust_core/`. Agent orchestration, LLM interaction, and tool implementation belong in `python_shell/`.
- Verify the current state of any file before proposing edits. Do not assume file contents, function signatures, or variable states.
- Prefer small, reversible atomic changes over sweeping architectural updates.
- Rust edition is 2024. Python requires >=3.9.
- CPG analysis is Python-only for now. Non-Python languages get empty CPG nodes but still participate in the file-level graph.
- When adding new side-maps to `CpgLayer`, you must handle swap-remove remapping in `remove_file()`.
