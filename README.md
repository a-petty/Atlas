# Atlas

A local-first autonomous coding agent that builds symbol-aware semantic graphs of repositories and provides intelligent context to LLMs for code modifications.

Atlas combines a high-performance Rust core (Tree-sitter parsing, graph algorithms, file watching) with a Python orchestration layer (agent logic, LLM interaction, CLI) to understand codebases at both the file and sub-file level.

## Features

- **Dual-graph architecture** — A file-level dependency graph (PageRank-scored) plus an optional Code Property Graph (CPG) overlay with control flow, data flow, and call graph edges
- **Multi-language support** — Tree-sitter parsing for Python, Rust, JavaScript, TypeScript, Go, and Java
- **Incremental updates** — 4-tier change classification avoids unnecessary recomputation when files change
- **Anchor & Expand context assembly** — Vector search finds relevant code, then graph traversal pulls in dependencies within a token budget
- **Live file watching** — Keeps the graph in sync as you edit, with debounced filesystem events
- **Syntax-aware tool use** — The "Reflexive Sensory Loop" validates syntax before saving files, catching errors before they propagate

## Quick Start

### Prerequisites

- Python >= 3.9
- Rust toolchain (install via [rustup](https://rustup.rs/))
- [Maturin](https://www.maturin.rs/) (`pip install maturin`)
- [Ollama](https://ollama.ai/) (for local LLM inference)

### Installation

```bash
git clone https://github.com/a-petty/Atlas.git
cd Atlas

# Create and activate a virtual environment
python -m venv .venv
source .venv/bin/activate

# Install Python dependencies
pip install -e ".[dev]"

# Build the Rust core as a Python extension
maturin develop
```

### Usage

```bash
# Watch a repository — builds the graph and keeps it updated
atlas watch /path/to/repo

# One-shot query against a repository
atlas query "Explain the authentication flow" -p /path/to/repo

# Interactive chat session
atlas chat -p /path/to/repo

# Specify a different Ollama model
atlas query "Find all unused imports" -p /path/to/repo --model codellama
```

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  Python Shell (python_shell/atlas/)                 │
│  ┌──────────┐ ┌──────────┐ ┌──────┐ ┌────────────┐  │
│  │  Agent   │ │ Context  │ │ LLM  │ │   Tools    │  │
│  │          │ │ Manager  │ │Client│ │ (read/write│  │
│  │ (orchest-│ │ (anchor  │ │(Olla-│ │  + syntax  │  │
│  │  rator)  │ │ +expand) │ │  ma) │ │  checking) │  │
│  └────┬─────┘ └────┬─────┘ └──────┘ └────────────┘  │
│       │             │          PyO3 boundary        │
├───────┼─────────────┼───────────────────────────────┤
│  Rust Core (rust_core/src/)                         │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐             │
│  │ RepoGraph│ │ CpgLayer │ │  Parser  │             │
│  │ (file-   │ │ (CPG:    │ │ (tree-   │             │
│  │  level   │ │  CFG +   │ │  sitter, │             │
│  │  graph,  │ │  dataflow│ │  6 langs,│             │
│  │ PageRank)│ │  + calls)│ │  symbols)│             │
│  └──────────┘ └──────────┘ └──────────┘             │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐             │
│  │ Symbol   │ │ Import   │ │ Watcher  │             │
│  │  Index   │ │ Resolver │ │ (notify) │             │
│  └──────────┘ └──────────┘ └──────────┘             │
└─────────────────────────────────────────────────────┘
```

**Rust Core** handles all performance-critical work: parsing, graph construction, symbol indexing, import resolution, and file watching. It exposes a Python API via PyO3.

**Python Shell** orchestrates the agent loop: initializing the graph, assembling context for LLM queries, executing tool calls, and managing the CLI.

## Development

```bash
# Activate the virtual environment
source .venv/bin/activate

# Rebuild Rust core after changes (required before Python tests reflect Rust changes)
maturin develop

# Run Rust tests
cd rust_core && cargo test

# Run a specific Rust test file
cargo test --test test_callgraph

# Run a specific Rust test by name
cargo test --test test_callgraph test_resolve_same_file

# Run Python tests
pytest python_shell/tests/

# Run Rust benchmarks
cd rust_core && cargo bench
```

## License

All rights reserved.
