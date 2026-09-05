"""Benchmark C: Incremental Update Latency.

Measures wall-clock time for graph.update_file() across the four update tiers:
- Local: content changed, symbols/imports unchanged
- FileScope: definitions changed
- GraphScope: imports changed
- FullRebuild: complete graph reconstruction from scratch

Synthetic changes are applied to trigger each tier, then reverted before the
next measurement to maintain consistent starting state.
"""

from pathlib import Path
from typing import List, Dict, Callable
from datetime import datetime, timezone

from benchmarks.common import (
    create_arg_parser, init_graph, Timer, BenchmarkResult,
    print_table, compute_percentiles, console, IGNORED_DIRS,
)

DEFAULT_ITERATIONS = 50
FULL_REBUILD_ITERATIONS = 5


def make_local_change(source: str, iteration: int) -> str:
    """Local tier: append a comment (changes content_hash only)."""
    return source.rstrip() + f"\n# bench-local-{iteration}\n"


def make_filescope_change(source: str, iteration: int) -> str:
    """FileScope tier: add a function definition (changes definitions_hash)."""
    return source.rstrip() + f"\n\ndef _bench_func_{iteration}():\n    pass\n"


def make_graphscope_change(source: str, iteration: int) -> str:
    """GraphScope tier: add an import (changes imports_hash)."""
    return f"import sys as _bench_sys_{iteration}\n" + source


def pick_benchmark_file(graph, files: List[str]) -> str:
    """Pick a medium-sized Python file from the graph for benchmarking."""
    candidates = []
    for f in files:
        p = Path(f)
        if p.suffix != ".py":
            continue
        if not graph.has_file(f):
            continue
        try:
            line_count = p.read_text().count("\n")
            if 30 < line_count < 500:
                candidates.append((f, line_count))
        except Exception:
            continue

    if not candidates:
        for f in files:
            if Path(f).suffix == ".py" and graph.has_file(f):
                return f
        raise RuntimeError("No Python files found in graph")

    # Prefer a file near 150 lines
    candidates.sort(key=lambda x: abs(x[1] - 150))
    return candidates[0][0]


def benchmark_tier(
    graph,
    file_path: str,
    original_source: str,
    change_fn: Callable[[str, int], str],
    tier_name: str,
    iterations: int,
) -> Dict:
    """Benchmark a single update tier, returning latency statistics."""
    latencies = []

    for i in range(iterations):
        modified = change_fn(original_source, i)

        with Timer() as t:
            graph.update_file(file_path, modified)

        latencies.append(t.elapsed * 1000)  # ms

        # Restore original for next iteration
        graph.update_file(file_path, original_source)

    stats = compute_percentiles(latencies)
    stats["tier"] = tier_name
    stats["iterations"] = len(latencies)
    stats["file"] = file_path
    return stats


def benchmark_full_rebuild(repo_path: str, files: List[str], iterations: int) -> Dict:
    """Benchmark full graph rebuild from scratch."""
    from atlas.semantic_engine import RepoGraph

    latencies = []
    for _ in range(iterations):
        with Timer() as t:
            g = RepoGraph(repo_path, ignored_dirs=IGNORED_DIRS)
            g.build_complete(files)
            g.ensure_pagerank_up_to_date()

        latencies.append(t.elapsed * 1000)

    stats = compute_percentiles(latencies)
    stats["tier"] = "FullRebuild"
    stats["iterations"] = len(latencies)
    stats["file"] = "(entire graph)"
    return stats


def run_benchmark(repo_path: str, iterations: int = DEFAULT_ITERATIONS) -> BenchmarkResult:
    """Run incremental latency benchmark across all tiers."""
    repo_path = str(Path(repo_path).resolve())
    graph, files = init_graph(repo_path)

    target_file = pick_benchmark_file(graph, files)
    original_source = Path(target_file).read_text()

    console.print(f"[bold]Target file:[/bold] {target_file}")
    console.print(f"[bold]Lines:[/bold] {original_source.count(chr(10)) + 1}")
    console.print(f"[bold]Iterations per tier:[/bold] {iterations}\n")

    result = BenchmarkResult(
        benchmark_name="incremental_latency",
        repo_path=repo_path,
        file_count=len(files),
        timestamp=datetime.now(timezone.utc).isoformat(),
        metadata={"target_file": target_file, "iterations": iterations},
    )

    tier_configs = [
        ("Local", make_local_change),
        ("FileScope", make_filescope_change),
        ("GraphScope", make_graphscope_change),
    ]

    for tier_name, change_fn in tier_configs:
        console.print(f"  Benchmarking [bold]{tier_name}[/bold]...")
        stats = benchmark_tier(
            graph, target_file, original_source,
            change_fn, tier_name, iterations,
        )
        result.measurements.append(stats)

    console.print(f"  Benchmarking [bold]FullRebuild[/bold] ({FULL_REBUILD_ITERATIONS} iterations)...")
    rebuild_stats = benchmark_full_rebuild(repo_path, files, FULL_REBUILD_ITERATIONS)
    result.measurements.append(rebuild_stats)

    # Restore original content
    graph.update_file(target_file, original_source)

    # Print summary
    rows = []
    for m in result.measurements:
        rows.append([
            m["tier"],
            f'{m["median"]:.2f}',
            f'{m["p95"]:.2f}',
            f'{m["p99"]:.2f}',
            f'{m["min"]:.2f}',
            f'{m["max"]:.2f}',
            str(m["iterations"]),
        ])

    console.print()
    print_table(
        "Incremental Update Latency (ms)",
        ["Tier", "Median", "p95", "p99", "Min", "Max", "N"],
        rows,
    )

    return result


def main():
    parser = create_arg_parser("Benchmark C: Incremental Update Latency")
    parser.add_argument("--iterations", type=int, default=DEFAULT_ITERATIONS)
    args = parser.parse_args()
    result = run_benchmark(args.repo, args.iterations)
    if args.output:
        result.save(Path(args.output))
        console.print(f"Results saved to {args.output}")
    console.print("[bold green]Benchmark complete.[/bold green]")


if __name__ == "__main__":
    main()
