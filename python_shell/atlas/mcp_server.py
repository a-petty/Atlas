"""
Atlas MCP Server — Exposes Atlas's semantic graph intelligence as MCP tools.

Provides 12 tools for graph-aware code intelligence: architecture maps,
dependency analysis, call graphs, semantic search, and context assembly.

Usage:
    atlas-mcp --project-root /path/to/repo
    atlas-mcp --project-root /path/to/repo --verbose
"""

import sys
import argparse
import logging
from pathlib import Path
from typing import Optional

# ---------------------------------------------------------------------------
# Import guards
# ---------------------------------------------------------------------------
try:
    from mcp.server.fastmcp import FastMCP
except ImportError:
    print(
        "ERROR: The 'mcp' package is not installed.\n"
        "Install it with: pip install -e '.[mcp]'",
        file=sys.stderr,
    )
    sys.exit(1)

try:
    from atlas.semantic_engine import (
        RepoGraph,
        scan_repository,
        create_skeleton_from_source,
    )
except ImportError as e:
    print(
        "ERROR: Atlas semantic engine not found. Build with: maturin develop\n"
        f"  Details: {e}",
        file=sys.stderr,
    )
    sys.exit(1)

# ---------------------------------------------------------------------------
# Logging — all output to stderr (required for stdio transport)
# ---------------------------------------------------------------------------
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    stream=sys.stderr,
)
log = logging.getLogger("atlas.mcp")

# ---------------------------------------------------------------------------
# MCP server instance
# ---------------------------------------------------------------------------
mcp = FastMCP(
    "Atlas",
    instructions="Semantic graph intelligence for codebases — architecture maps, "
    "dependency analysis, call graphs, and optimized context assembly.",
)

# ---------------------------------------------------------------------------
# Global state
# ---------------------------------------------------------------------------
_graph: Optional[RepoGraph] = None
_project_root: Optional[Path] = None
_cpg_enabled: bool = False
_embedding_manager = None  # Lazy: atlas.embeddings.EmbeddingManager
_context_manager = None    # Lazy: atlas.context.ContextManager

IGNORED_DIRS = {"node_modules", "target", ".git", "__pycache__", "dist", "build", ".venv", "venv"}


# ---------------------------------------------------------------------------
# Initialization helpers
# ---------------------------------------------------------------------------
def _initialize_graph(project_root: Path) -> None:
    """Scan repository, build file-level graph, compute PageRank."""
    global _graph, _project_root

    _project_root = project_root.resolve()
    log.info("Initializing graph for %s", _project_root)

    _graph = RepoGraph(str(_project_root))

    files = scan_repository(str(_project_root), ignored_dirs=list(IGNORED_DIRS))
    log.info("Scanned %d files", len(files))

    _graph.build_complete(files)
    _graph.ensure_pagerank_up_to_date()

    stats = _graph.get_statistics()
    log.info(
        "Graph ready: %d files, %d edges, %d symbols",
        stats.node_count,
        stats.edge_count,
        stats.total_definitions,
    )


def _ensure_cpg() -> None:
    """Lazily enable CPG overlay on first call that needs it."""
    global _cpg_enabled
    if _cpg_enabled:
        return
    if _graph is None:
        return
    log.info("Enabling CPG overlay (first CPG tool call)...")
    _graph.enable_cpg()
    _cpg_enabled = True
    log.info("CPG enabled")


def _ensure_embeddings() -> None:
    """Lazily initialize EmbeddingManager and ContextManager."""
    global _embedding_manager, _context_manager
    if _embedding_manager is not None:
        return
    log.info("Initializing embedding manager (first semantic search call)...")
    from atlas.embeddings import EmbeddingManager
    from atlas.context import ContextManager

    _embedding_manager = EmbeddingManager()
    _context_manager = ContextManager(_graph, _embedding_manager)
    log.info("Embedding manager ready")


def _normalize_path(file_path: str) -> str:
    """Accept absolute or project-relative path, return canonical absolute."""
    p = Path(file_path)
    if not p.is_absolute():
        p = _project_root / p
    canonical = p.resolve()
    # Verify it's within the project
    try:
        canonical.relative_to(_project_root)
    except ValueError:
        raise ValueError(f"Path {canonical} is outside project root {_project_root}")
    return str(canonical)


def _to_relative(abs_path: str) -> str:
    """Convert an absolute path back to project-relative for output."""
    try:
        return str(Path(abs_path).relative_to(_project_root))
    except ValueError:
        return abs_path


# ---------------------------------------------------------------------------
# Tool definitions
# ---------------------------------------------------------------------------

@mcp.tool()
def atlas_status() -> str:
    """Get graph statistics and readiness status. Call this first to verify Atlas is ready."""
    if _graph is None:
        return "ERROR: Graph not initialized"
    try:
        stats = _graph.get_statistics()
        lines = [
            "Atlas Semantic Graph Status",
            f"  Project root: {_project_root}",
            f"  Files indexed: {stats.node_count}",
            f"  Dependency edges: {stats.edge_count}",
            f"    Import edges: {stats.import_edges}",
            f"    Symbol usage edges: {stats.symbol_edges}",
            f"  Symbol definitions: {stats.total_definitions}",
            f"  CPG enabled: {_cpg_enabled}",
            f"  Embeddings loaded: {_embedding_manager is not None}",
        ]
        return "\n".join(lines)
    except Exception as e:
        return f"ERROR: {e}"


@mcp.tool()
def get_repository_map(max_files: int = 50) -> str:
    """Get a PageRank-ordered architecture overview of the repository.

    Returns the most architecturally important files with their dependencies
    and symbols, ordered by importance score. Use this to understand the
    overall structure before diving into specific files.

    Args:
        max_files: Maximum number of files to include (default 50).
    """
    if _graph is None:
        return "ERROR: Graph not initialized"
    try:
        return _graph.generate_map(max_files)
    except Exception as e:
        return f"ERROR: {e}"


@mcp.tool()
def get_dependencies(file_path: str) -> str:
    """Get outgoing dependencies for a file (what this file imports/uses).

    Args:
        file_path: Absolute or project-relative path to the file.
    """
    if _graph is None:
        return "ERROR: Graph not initialized"
    try:
        normalized = _normalize_path(file_path)
        deps = _graph.get_dependencies(normalized)
        if not deps:
            return f"No outgoing dependencies found for {_to_relative(normalized)}"
        lines = [f"Dependencies of {_to_relative(normalized)}:"]
        for dep_path, edge_kind in deps:
            lines.append(f"  {_to_relative(dep_path)} ({edge_kind})")
        return "\n".join(lines)
    except Exception as e:
        return f"ERROR: {e}"


@mcp.tool()
def get_dependents(file_path: str) -> str:
    """Get incoming dependents for a file (what depends on this file — blast radius).

    Args:
        file_path: Absolute or project-relative path to the file.
    """
    if _graph is None:
        return "ERROR: Graph not initialized"
    try:
        normalized = _normalize_path(file_path)
        deps = _graph.get_dependents(normalized)
        if not deps:
            return f"No incoming dependents found for {_to_relative(normalized)}"
        lines = [f"Dependents of {_to_relative(normalized)} (files that depend on this):"]
        for dep_path, edge_kind in deps:
            lines.append(f"  {_to_relative(dep_path)} ({edge_kind})")
        return "\n".join(lines)
    except Exception as e:
        return f"ERROR: {e}"


@mcp.tool()
def get_top_ranked_files(limit: int = 20) -> str:
    """Get the most architecturally important files ranked by PageRank.

    Higher-ranked files are more central to the codebase — they are imported
    by many other files and define widely-used symbols.

    Args:
        limit: Number of files to return (default 20).
    """
    if _graph is None:
        return "ERROR: Graph not initialized"
    try:
        ranked = _graph.get_top_ranked_files(limit)
        if not ranked:
            return "No ranked files available"
        lines = ["Top files by architectural importance (PageRank):"]
        for i, (path, rank) in enumerate(ranked, 1):
            lines.append(f"  {i:3d}. {_to_relative(path)} (score: {rank:.4f})")
        return "\n".join(lines)
    except Exception as e:
        return f"ERROR: {e}"


@mcp.tool()
def find_relevant_files(query: str, top_n: int = 10) -> str:
    """Find files most relevant to a natural language query using semantic search.

    Uses vector embeddings to find files whose content is semantically similar
    to the query. Good for finding code related to a concept or feature.

    Args:
        query: Natural language description of what you're looking for.
        top_n: Number of results to return (default 10).
    """
    if _graph is None:
        return "ERROR: Graph not initialized"
    try:
        _ensure_embeddings()
        all_files = [Path(p) for p, _ in _graph.get_top_ranked_files(1000)]
        results = _embedding_manager.find_relevant_files(query, all_files, top_n=top_n)
        if not results:
            return f"No relevant files found for: {query}"
        lines = [f"Files relevant to '{query}':"]
        for i, path in enumerate(results, 1):
            lines.append(f"  {i}. {_to_relative(str(path))}")
        return "\n".join(lines)
    except Exception as e:
        return f"ERROR: {e}"


@mcp.tool()
def assemble_context(query: str) -> str:
    """Assemble optimized code context for a query using Anchor & Expand.

    This is Atlas's core intelligence: it finds relevant files via semantic
    search (anchor), then expands through the dependency graph to pull in
    related code. Returns a three-tier context: repository map, full file
    content for key files, and skeletons for architectural context.

    Args:
        query: The coding task or question to assemble context for.
    """
    if _graph is None:
        return "ERROR: Graph not initialized"
    try:
        _ensure_embeddings()
        return _context_manager.assemble_context(query, files_in_scope=[])
    except Exception as e:
        return f"ERROR: {e}"


@mcp.tool()
def get_file_symbols(file_path: str) -> str:
    """Get all functions, methods, and classes defined in a file with their signatures.

    Includes parameter names, type annotations, return types, and docstrings.
    Requires CPG (enabled automatically on first call).

    Args:
        file_path: Absolute or project-relative path to the file.
    """
    if _graph is None:
        return "ERROR: Graph not initialized"
    try:
        _ensure_cpg()
        normalized = _normalize_path(file_path)
        symbols = _graph.get_functions_in_file(normalized)
        if not symbols:
            return f"No symbols found in {_to_relative(normalized)}"
        lines = [f"Symbols in {_to_relative(normalized)}:"]
        for sym in symbols:
            kind = sym["kind"]
            name = sym["name"]
            params = sym.get("parameters", [])
            param_strs = []
            for p in params:
                s = p["name"]
                if p.get("type_annotation"):
                    s += f": {p['type_annotation']}"
                if p.get("default_value"):
                    s += f" = {p['default_value']}"
                param_strs.append(s)
            sig = f"{kind} {name}({', '.join(param_strs)})"
            ret = sym.get("return_type")
            if ret:
                sig += f" -> {ret}"
            parent = sym.get("parent_class")
            if parent:
                sig += f"  [in class {parent}]"
            lines.append(f"  L{sym['start_line']}-{sym['end_line']}: {sig}")
            doc = sym.get("docstring")
            if doc:
                first_line = doc.strip().split("\n")[0]
                lines.append(f"    {first_line}")
        return "\n".join(lines)
    except Exception as e:
        return f"ERROR: {e}"


@mcp.tool()
def get_callees(file_path: str, function_name: str) -> str:
    """Get all functions called by a given function (outgoing call graph).

    Args:
        file_path: Absolute or project-relative path to the file containing the function.
        function_name: Name of the function to analyze.
    """
    if _graph is None:
        return "ERROR: Graph not initialized"
    try:
        _ensure_cpg()
        normalized = _normalize_path(file_path)
        callees = _graph.get_callees(normalized, function_name)
        if not callees:
            return f"No callees found for {function_name} in {_to_relative(normalized)}"
        lines = [f"Functions called by {function_name}:"]
        for callee in callees:
            lines.append(f"  {callee['name']} ({_to_relative(callee['file'])}:{callee['line']})")
        return "\n".join(lines)
    except Exception as e:
        return f"ERROR: {e}"


@mcp.tool()
def get_callers(file_path: str, function_name: str) -> str:
    """Get all functions that call a given function (incoming call graph).

    Args:
        file_path: Absolute or project-relative path to the file containing the function.
        function_name: Name of the function to analyze.
    """
    if _graph is None:
        return "ERROR: Graph not initialized"
    try:
        _ensure_cpg()
        normalized = _normalize_path(file_path)
        callers = _graph.get_callers(normalized, function_name)
        if not callers:
            return f"No callers found for {function_name} in {_to_relative(normalized)}"
        lines = [f"Functions that call {function_name}:"]
        for caller in callers:
            lines.append(f"  {caller['name']} ({_to_relative(caller['file'])}:{caller['line']})")
        return "\n".join(lines)
    except Exception as e:
        return f"ERROR: {e}"


@mcp.tool()
def get_file_skeleton(file_path: str) -> str:
    """Get function/class signatures without implementation bodies.

    Useful for understanding a file's API surface without reading the full source.

    Args:
        file_path: Absolute or project-relative path to the file.
    """
    if _graph is None:
        return "ERROR: Graph not initialized"
    try:
        normalized = _normalize_path(file_path)
        skeleton = _graph.get_skeleton(normalized)
        if not skeleton or not skeleton.strip():
            return f"No skeleton available for {_to_relative(normalized)}"
        return f"Skeleton of {_to_relative(normalized)}:\n\n{skeleton}"
    except Exception as e:
        return f"ERROR: {e}"


@mcp.tool()
def atlas_refresh() -> str:
    """Re-scan the repository and rebuild the graph from scratch.

    Use this after significant file system changes (branch switches, large
    merges, etc.) to ensure the graph is up to date.
    """
    global _cpg_enabled, _embedding_manager, _context_manager
    if _project_root is None:
        return "ERROR: Project root not set"
    try:
        _cpg_enabled = False
        _embedding_manager = None
        _context_manager = None
        _initialize_graph(_project_root)
        stats = _graph.get_statistics()
        return (
            f"Graph refreshed: {stats.node_count} files, "
            f"{stats.edge_count} edges, {stats.total_definitions} symbols"
        )
    except Exception as e:
        return f"ERROR: {e}"


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------
def main():
    parser = argparse.ArgumentParser(
        prog="atlas-mcp",
        description="Atlas MCP Server — semantic graph intelligence for Claude Code",
    )
    parser.add_argument(
        "--project-root",
        type=Path,
        default=Path("."),
        help="Path to the repository to analyze (default: current directory)",
    )
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Enable debug logging",
    )
    args = parser.parse_args()

    if args.verbose:
        logging.getLogger("atlas").setLevel(logging.DEBUG)
        log.setLevel(logging.DEBUG)

    project_root = args.project_root.resolve()
    if not project_root.is_dir():
        print(f"ERROR: {project_root} is not a directory", file=sys.stderr)
        sys.exit(1)

    _initialize_graph(project_root)
    mcp.run(transport="stdio")


if __name__ == "__main__":
    main()
