"""Benchmark A: Token Efficiency.

Compares Atlas's assemble_context (Anchor & Expand with three-tier budgeting)
against two baselines at varying token budgets:

1. Flat embedding search — pure vector similarity, no graph expansion
2. Random file selection — statistical lower bound

Measures tokens used, files included, and wall-clock time for each strategy.
"""

import re
import random
from pathlib import Path
from typing import List, Dict
from datetime import datetime, timezone

from benchmarks.common import (
    create_arg_parser, init_graph, get_encoder, count_tokens,
    Timer, BenchmarkResult, print_table, console,
)

DEFAULT_BUDGETS = [8_000, 16_000, 32_000, 64_000]
DEFAULT_QUERY = "How does the context assembly system work?"


def parse_atlas_output(context_str: str) -> Dict[str, List[str]]:
    """Parse assemble_context() output to extract files per section.

    The output format uses delimiters:
        ================================================================================
        SECTION_LABEL
        ================================================================================
        # /path/to/file.py (FULL CONTENT)
        ...content...

    Returns dict mapping tier names to lists of file paths.
    """
    tiers = {
        "map": [],
        "explicit": [],
        "anchor": [],
        "neighborhood": [],
        "skeleton": [],
    }

    file_pattern = re.compile(r"^# (.+?) \((FULL CONTENT|SKELETON)\)", re.MULTILINE)

    # Split on 80-char '=' delimiter lines
    sections = re.split(r"={60,}\n(.+?)\n={60,}", context_str)

    for i in range(1, len(sections), 2):
        if i + 1 >= len(sections):
            break
        section_name = sections[i].strip()
        section_body = sections[i + 1]

        files = [m.group(1) for m in file_pattern.finditer(section_body)]

        if "REPOSITORY_MAP" in section_name:
            pass  # Map section doesn't have individual file entries
        elif "EXPLICIT" in section_name:
            tiers["explicit"] = files
        elif "ANCHOR" in section_name:
            tiers["anchor"] = files
        elif "NEIGHBORHOOD" in section_name:
            tiers["neighborhood"] = files
        elif "ARCHITECTURAL" in section_name or "EXTENDED" in section_name:
            tiers["skeleton"] = files

    return tiers


def _measurement(strategy, budget, result, elapsed):
    from collections import Counter
    files = {c["file"] for c in result.chunks}
    per_tier = Counter()
    for file in files:
        chunk = next(c for c in result.chunks if c["file"] == file)
        per_tier[chunk["selection_reason"]] += 1
    return {"strategy": strategy, "budget": budget,
            "total_tokens_used": result.counts["tokens"],
            "files_included": len(files), "files_per_tier": dict(per_tier),
            "file_list": sorted(files), "wall_clock_ms": round(elapsed * 1000, 1),
            "omissions": result.counts["omission_reasons"]}


def atlas_strategy(graph, embedding_manager, budget, query, encoder):
    from atlas.context import ContextManager
    manager = ContextManager(graph, embedding_manager, max_tokens=budget)
    with Timer() as timer:
        result = manager.assemble_context_result(query, [])
    return _measurement("atlas", budget, result, timer.elapsed)


def flat_embedding_strategy(graph, embedding_manager, budget, query, encoder):
    from atlas.context import ContextManager
    manager = ContextManager(graph, embedding_manager, max_tokens=budget)
    files = [Path(p) for p, _ in graph.get_top_ranked_files(graph.get_statistics().node_count)]
    with Timer() as timer:
        relevant = embedding_manager.find_relevant_files(query, files, top_n=len(files))
        result = manager.pack_ranked_context_result(query, [(p, "embedding") for p in relevant])
    return _measurement("flat_embedding", budget, result, timer.elapsed)


def random_strategy(graph, budget, encoder, query=DEFAULT_QUERY):
    from atlas.context import ContextManager
    manager = ContextManager(graph, None, max_tokens=budget)
    files = [Path(p) for p, _ in graph.get_top_ranked_files(graph.get_statistics().node_count)]
    random.Random(20260905).shuffle(files)
    with Timer() as timer:
        result = manager.pack_ranked_context_result(query, [(p, "random") for p in files])
    return _measurement("random", budget, result, timer.elapsed)


def run_benchmark(
    repo_path: str,
    budgets: List[int] = DEFAULT_BUDGETS,
    query: str = DEFAULT_QUERY,
) -> BenchmarkResult:
    """Run the full token efficiency benchmark."""
    repo_path = str(Path(repo_path).resolve())
    graph, files = init_graph(repo_path)
    encoder = get_encoder()

    from atlas.embeddings import EmbeddingManager
    emb = EmbeddingManager(repo_graph=graph)

    result = BenchmarkResult(
        benchmark_name="token_efficiency",
        repo_path=repo_path,
        file_count=len(files),
        timestamp=datetime.now(timezone.utc).isoformat(),
        metadata={"query": query, "budgets": budgets},
    )

    for budget in budgets:
        console.print(f"\n[bold]Budget: {budget:,} tokens[/bold]")

        atlas_result = atlas_strategy(graph, emb, budget, query, encoder)
        embedding_result = flat_embedding_strategy(graph, emb, budget, query, encoder)
        random_result = random_strategy(graph, budget, encoder, query)

        result.measurements.append({
            "budget": budget,
            "atlas": atlas_result,
            "flat_embedding": embedding_result,
            "random": random_result,
        })

        rows = []
        for r in [atlas_result, embedding_result, random_result]:
            rows.append([
                r["strategy"],
                f'{r["total_tokens_used"]:,}',
                str(r["files_included"]),
                f'{r["wall_clock_ms"]}ms',
            ])
        print_table(
            f"Token Budget: {budget:,}",
            ["Strategy", "Tokens Used", "Files", "Time"],
            rows,
        )

    return result


def main():
    parser = create_arg_parser("Benchmark A: Token Efficiency")
    parser.add_argument("--budgets", type=str, default="8000,16000,32000,64000")
    parser.add_argument("--query", type=str, default=DEFAULT_QUERY)
    args = parser.parse_args()

    budgets = [int(b) for b in args.budgets.split(",")]
    result = run_benchmark(args.repo, budgets, args.query)
    if args.output:
        result.save(Path(args.output))
        console.print(f"Results saved to {args.output}")
    console.print("\n[bold green]Benchmark complete.[/bold green]")


if __name__ == "__main__":
    main()
