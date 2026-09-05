"""Atlas MCP transport over a restartable, automatically refreshed repository session."""
import argparse
import json
import logging
from pathlib import Path
import sys
from typing import Optional
from mcp.server.fastmcp import FastMCP
from atlas.session import RepositorySession, IGNORED_DIRS, MAX_RESULT_CHARS

logging.basicConfig(level=logging.INFO, stream=sys.stderr)
log = logging.getLogger("atlas.mcp")
mcp = FastMCP("Atlas", instructions="Repository analysis from coherent source generations. "
             "Each query reconciles edits automatically. Lists are paginated; use next_cursor "
             "only with the same query and generation. Context includes delivered source spans.")
_project_root: Optional[Path] = None
_session: Optional[RepositorySession] = None


def _get_session():
    global _session
    if _project_root is None:
        raise RuntimeError("Project root not set")
    if _session is None or _session.root != _project_root.resolve():
        if _session is not None: _session.close()
        _session = RepositorySession(_project_root)
    return _session


def _render(result, response_format="text"):
    if response_format not in {"text", "json"}:
        raise ValueError("response_format must be text or json")
    if response_format == "json":
        output = json.dumps(result)
    elif "text" in result:
        output = result["text"]
        coverage = result.get("coverage", {})
        if coverage.get("invalid_count"):
            output += "\nCOVERAGE: " + json.dumps(coverage)
    elif "items" in result:
        lines = [f"Source generation: {result['generation']}", f"Results in this page: {len(result['items'])} of {result['total']}"]
        for item in result["items"]:
            if "text" in item:
                lines.append(item["text"])
            elif "file" in item:
                detail = item.get("qualified_name", item.get("name", item.get("kind", "")))
                if "line" in item: detail += f" (line {item['line']})"
                for metric in ("score", "rank"):
                    if metric in item: detail += f" {metric}={item[metric]:.6f}"
                lines.append(f"{item['file']}: {detail}".rstrip(": "))
            else:
                lines.append(json.dumps(item))
        if result.get("query_resolution"):
            lines.append("Query resolution: " + json.dumps(result["query_resolution"]))
        if result.get("next_cursor"):
            lines.append("Next cursor: " + result["next_cursor"])
        lines.append("Coverage: " + json.dumps(result["coverage"]))
        output = "\n".join(lines)
    else:
        output = json.dumps(result, indent=2)
    if len(output) > MAX_RESULT_CHARS:
        raise ValueError("Result exceeds transport capacity; request a smaller page")
    return output


async def _request(operation, response_format="text", **params):
    try:
        result = await _get_session().arequest(operation, **params)
        return _render(result, response_format)
    except Exception as exc:
        error = f"ERROR: {exc}"
        return error if len(error) <= MAX_RESULT_CHARS else "ERROR: Diagnostic exceeds transport capacity; inspect the server log for details."


def _load_atlas_config(project_root: Path):
    """Load `.atlas.toml` overrides for graph construction.

    Returns `(source_roots, languages)`, each `None` when not configured.
    `languages`, when set, pins the resolver groups and bypasses
    auto-detection — useful for repos where detection picks up a language
    the user wants to ignore (e.g. vendored polyglot examples) or wants to
    extend (e.g. force a JS/TS resolver during a partial migration).
    """
    toml_file = project_root / ".atlas.toml"
    if not toml_file.exists():
        return None, None
    try:
        import tomllib
    except ImportError:
        try:
            import tomli as tomllib  # type: ignore[no-redef]
        except ImportError:
            log.warning(".atlas.toml found but neither tomllib nor tomli available. Ignoring.")
            return None, None
    try:
        with open(toml_file, "rb") as f:
            config = tomllib.load(f)
    except Exception as e:
        log.warning("Failed to parse .atlas.toml: %s", e)
        return None, None

    project = config.get("project", {})

    roots = project.get("source_roots")
    if roots is not None and not (isinstance(roots, list) and all(isinstance(r, str) for r in roots)):
        log.warning(".atlas.toml: `project.source_roots` must be a list of strings; ignoring")
        roots = None
    elif roots:
        log.info("Loaded source roots from .atlas.toml: %s", roots)
    else:
        roots = None

    languages = project.get("languages")
    if languages is not None and not (isinstance(languages, list) and all(isinstance(l, str) for l in languages)):
        log.warning(".atlas.toml: `project.languages` must be a list of strings; ignoring")
        languages = None
    elif languages:
        log.info("Loaded languages from .atlas.toml: %s — bypassing auto-detection", languages)
    else:
        languages = None

    return roots, languages


_LANGUAGE_DISPLAY_NAMES = {
    "python": "Python",
    "rust": "Rust",
    "javascript": "JavaScript",
    "javascript_jsx": "JSX",
    "typescript": "TypeScript",
    "typescript_tsx": "TSX",
    "go": "Go",
    "unknown": "Unknown",
}

_RESOLVER_DISPLAY_NAMES = {
    "python": "Python",
    "javascript_typescript": "JS/TS",
}


def _format_coverage_block(stats) -> list:
    """Render the Coverage section for `atlas_status`.

    The 'No resolver available' line is the load-bearing piece — it tells
    callers (humans and LLMs) that import analysis is unavailable for files
    in some language present in the repo, rather than leaving them to infer
    "0 imports must mean nothing depends on this" from a silent gap.
    """
    lines = ["  Coverage:"]

    resolvers = list(stats.registered_resolvers)
    if resolvers:
        rendered = ", ".join(_RESOLVER_DISPLAY_NAMES.get(r, r) for r in resolvers)
    else:
        rendered = "(none — graph has no import resolvers)"
    lines.append(f"    Registered resolvers: {rendered}")

    counts = dict(stats.file_counts_by_language)
    if counts:
        # Descending count, alphabetical name tiebreak — matches the Rust-side
        # detection ordering for resolver groups.
        sorted_counts = sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))
        rendered = ", ".join(
            f"{_LANGUAGE_DISPLAY_NAMES.get(lang, lang)} ({n})"
            for lang, n in sorted_counts
        )
        lines.append(f"    Files by language: {rendered}")

    unsupported = dict(stats.unsupported_language_counts)
    if unsupported:
        sorted_unsupported = sorted(unsupported.items(), key=lambda kv: (-kv[1], kv[0]))
        rendered = ", ".join(
            f"{_LANGUAGE_DISPLAY_NAMES.get(lang, lang)} ({n} file{'s' if n != 1 else ''})"
            for lang, n in sorted_unsupported
        )
        lines.append(f"    No resolver available: {rendered}")

    return lines



@mcp.tool()
async def atlas_status(response_format: str = "text") -> str:
    """Read coverage, generation, and preparation progress without forcing embeddings."""
    session = _get_session()
    if session.busy:
        return _render({"phase": "preparing", "complete": False, "progress": session.progress}, response_format)
    return await _request("status", response_format)


@mcp.tool()
async def get_repository_map(max_files: int = 50, cursor: Optional[str] = None, response_format: str = "text") -> str:
    """Read the architecture ordered by PageRank; continue large maps with next_cursor."""
    return await _request("map", response_format, limit=max_files, cursor=cursor)


@mcp.tool()
async def get_dependencies(file_path: str, limit: int = 100, cursor: Optional[str] = None, response_format: str = "text") -> str:
    """Read outgoing imports and symbol-use evidence, retaining both kinds per file pair."""
    return await _request("dependencies", response_format, file_path=file_path, limit=limit, cursor=cursor)


@mcp.tool()
async def get_dependents(file_path: str, limit: int = 100, cursor: Optional[str] = None, response_format: str = "text") -> str:
    """Read incoming dependencies from the latest accepted source generation."""
    return await _request("dependents", response_format, file_path=file_path, limit=limit, cursor=cursor)


@mcp.tool()
async def get_top_ranked_files(limit: int = 20, cursor: Optional[str] = None, response_format: str = "text") -> str:
    """Read architectural importance with generation-bound continuation."""
    return await _request("ranked", response_format, limit=limit, cursor=cursor)


@mcp.tool()
async def find_relevant_files(query: str, top_n: int = 10, cursor: Optional[str] = None, response_format: str = "text") -> str:
    """Rank all eligible files semantically. Cold preparation completes before ranking is returned."""
    return await _request("search", response_format, query=query, limit=top_n, cursor=cursor)


@mcp.tool()
async def assemble_context(query: str, files_in_scope: Optional[list[str]] = None, max_tokens: int = 12000, include_map: bool = True, response_format: str = "text") -> str:
    """Pack map, full sources and complete skeleton declarations within token/transport budgets.

    JSON includes the exact delivered source spans, modes, reasons and omissions.
    Explicit files take priority. Every part comes from the reported generation.
    """
    return await _request("context", response_format, query=query, files_in_scope=files_in_scope or [], max_tokens=max_tokens, include_map=include_map)


@mcp.tool()
async def get_file_symbols(file_path: str, limit: int = 100, cursor: Optional[str] = None, response_format: str = "text") -> str:
    """Read declared symbols without building CFG or data-flow overlays."""
    return await _request("symbols", response_format, file_path=file_path, limit=limit, cursor=cursor)


@mcp.tool()
async def get_callees(file_path: str, function_name: str, limit: int = 100, cursor: Optional[str] = None, response_format: str = "text") -> str:
    """Read statically resolved calls. Use a qualified name for duplicate method names."""
    return await _request("callees", response_format, file_path=file_path, function_name=function_name, limit=limit, cursor=cursor)


@mcp.tool()
async def get_callers(file_path: str, function_name: str, limit: int = 100, cursor: Optional[str] = None, response_format: str = "text") -> str:
    """Read callers across the complete lightweight index, independently of CFG load order."""
    return await _request("callers", response_format, file_path=file_path, function_name=function_name, limit=limit, cursor=cursor)


@mcp.tool()
async def get_file_skeleton(file_path: str, limit: int = 100, cursor: Optional[str] = None, response_format: str = "text") -> str:
    """Read API-preserving declarations for Python, JS/TS, Rust or Go with retained spans."""
    return await _request("skeleton", response_format, file_path=file_path, limit=limit, cursor=cursor)


@mcp.tool()
async def atlas_refresh(response_format: str = "text") -> str:
    """Force a fresh configuration epoch and graph rebuild; routine edits refresh automatically."""
    return await _request("refresh", response_format)


@mcp.tool()
async def diag_full_cpg_build() -> str:
    """Explicit diagnostic: build CFG/data-flow overlays in the bounded worker process."""
    return await _request("cpg_diagnostic")


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

    global _project_root

    project_root = args.project_root.resolve()
    if not project_root.is_dir():
        print(f"ERROR: {project_root} is not a directory", file=sys.stderr)
        sys.exit(1)

    _project_root = project_root
    log.info("Atlas MCP server starting (project: %s, graph builds on first tool call)", project_root)
    try:
        mcp.run(transport="stdio")
    finally:
        if _session is not None:
            _session.close()


if __name__ == "__main__":
    main()
