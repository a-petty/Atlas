# Contributing to Atlas

Thanks for your interest in contributing to Atlas! This guide will help you get started.

## Getting Started

### Prerequisites

- Python >= 3.10
- Rust toolchain ([rustup](https://rustup.rs/))
- [Maturin](https://www.maturin.rs/) (`pip install maturin`)

### Development Setup

```bash
git clone https://github.com/a-petty/Atlas.git
cd Atlas

# Create and activate a virtual environment
python -m venv .venv
source .venv/bin/activate

# Install Python dependencies (including dev tools)
pip install -e ".[dev]"

# Build the Rust core as a Python extension
maturin develop
```

After any change to Rust code, run `maturin develop` before testing from Python.

### Running Tests

```bash
# Rust tests
cd rust_core && cargo test

# Python tests
pytest python_shell/tests/

# Rust benchmarks
cd rust_core && cargo bench
```

## How to Contribute

### Reporting Bugs

Open an issue with:
- A clear title and description
- Steps to reproduce
- Expected vs. actual behavior
- Atlas version, OS, Python version, Rust version

### Suggesting Features

Open an issue tagged `enhancement` with:
- The problem you're trying to solve
- Your proposed solution
- Any alternatives you've considered

### Pull Requests

1. **Fork** the repository and create your branch from `main`.
2. **Write tests** for any new functionality. Rust tests go in `rust_core/tests/`, Python tests in `python_shell/tests/`.
3. **Run the full test suite** before submitting — both Rust and Python tests must pass.
4. **Follow existing code style**:
   - Rust: standard `rustfmt` formatting
   - Python: follow the patterns in existing modules
5. **Keep PRs focused** — one feature or fix per PR.
6. **Update documentation** if your change affects the public API or MCP tools.

### Areas Where Help Is Especially Welcome

- **CPG support for additional languages** — Currently Python-only. TypeScript and Rust are high-value targets.
- **Import resolution** — Extending to Go, Java, and Rust.
- **Benchmarks and evaluation** — Rigorous comparisons against baseline context strategies (e.g., on SWE-bench).
- **Documentation** — Tutorials, architecture deep-dives, example integrations.
- **Editor integrations** — Neovim, JetBrains MCP support, etc.

## Architecture Overview

If you're new to the codebase, start with:

1. [`How_Atlas_Works.md`](How_Atlas_Works.md) — Detailed walkthrough of the system
2. [`rust_core/src/graph.rs`](rust_core/src/graph.rs) — The core `RepoGraph` struct
3. [`python_shell/atlas/context.py`](python_shell/atlas/context.py) — The `ContextManager` and Anchor & Expand logic
4. [`python_shell/atlas/mcp_server.py`](python_shell/atlas/mcp_server.py) — MCP tool definitions

### Key Design Principles

- **All parsing goes through Tree-sitter** via the Rust core. Source code is never parsed in Python.
- **Conservative over aggressive** — e.g., the call graph only creates edges for unambiguously resolved calls. We prefer missing edges to false-positive edges.
- **Parallel-then-serial** — File parsing is parallelized via rayon; graph mutation is serialized (petgraph requires exclusive access).
- **Stateless analyzers** — `CfgBuilder`, `DataFlowAnalyzer`, and `CallGraphBuilder` are stateless structs with associated functions that mutate `CpgLayer` in-place.

## Code of Conduct

Be respectful. We're building something useful together, and everyone's contributions are valued. Harassment, personal attacks, and bad-faith engagement won't be tolerated.

## License

By contributing to Atlas, you agree that your contributions will be licensed under the [MIT License](LICENSE).