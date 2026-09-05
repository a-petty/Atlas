"""Benchmark D: PageRank vs Random Ordering.

Compares Atlas's PageRank-ranked file ordering against alphabetical and random
orderings. Ground truth is defined independently as files ranked by their
dependent count (number of incoming dependency edges).

Measures coverage (how many ground-truth important files appear in the top-k)
and precision (what fraction of the top-k selections are genuinely important).
"""

import random
from pathlib import Path
from typing import List, Set
from datetime import datetime, timezone

from benchmarks.common import (
    create_arg_parser, init_graph, BenchmarkResult, print_table, console,
)

DEFAULT_TOP_K_VALUES = [5, 10, 20, 50]
DEFAULT_RANDOM_TRIALS = 10


def compute_ground_truth(graph, files: List[str]) -> List[str]:
    """Rank files by dependent count (incoming edges), independent of PageRank."""
    dependent_counts = {}
    for f in files:
        if not graph.has_file(f):
            continue
        try:
            deps = graph.get_dependents(f)
            dependent_counts[f] = len(deps)
        except Exception:
            dependent_counts[f] = 0

    return sorted(
        dependent_counts.keys(),
        key=lambda f: (-dependent_counts[f], f),
    )


def pagerank_ordering(graph, files: List[str]) -> List[str]:
    ranked = graph.get_top_ranked_files(len(files))
    return [path for path, _score in ranked]


def alphabetical_ordering(files: List[str]) -> List[str]:
    return sorted(files)


def random_ordering(files: List[str]) -> List[str]:
    shuffled = list(files)
    random.shuffle(shuffled)
    return shuffled


def coverage_at_k(ordering: List[str], ground_truth_top: Set[str], k: int) -> float:
    """Fraction of ground_truth_top files present in ordering[:k]."""
    if not ground_truth_top:
        return 0.0
    return len(set(ordering[:k]) & ground_truth_top) / len(ground_truth_top)


def precision_at_k(ordering: List[str], ground_truth_top: Set[str], k: int) -> float:
    """Fraction of ordering[:k] that are in ground_truth_top."""
    if k == 0:
        return 0.0
    return len(set(ordering[:k]) & ground_truth_top) / k


def run_benchmark(
    repo_path: str,
    top_k_values: List[int] = DEFAULT_TOP_K_VALUES,
    random_trials: int = DEFAULT_RANDOM_TRIALS,
) -> BenchmarkResult:
    """Run the PageRank vs Random ordering benchmark."""
    repo_path = str(Path(repo_path).resolve())
    graph, files = init_graph(repo_path)

    graph_files = [f for f in files if graph.has_file(f)]
    console.print(f"[bold]Files in graph:[/bold] {len(graph_files)}\n")

    ground_truth = compute_ground_truth(graph, graph_files)
    pr_ordering = pagerank_ordering(graph, graph_files)
    alpha_ordering = alphabetical_ordering(graph_files)

    result = BenchmarkResult(
        benchmark_name="pagerank_ordering",
        repo_path=repo_path,
        file_count=len(graph_files),
        timestamp=datetime.now(timezone.utc).isoformat(),
        metadata={"top_k_values": top_k_values, "random_trials": random_trials},
    )

    for k in top_k_values:
        # Cap k to available files
        effective_k = min(k, len(graph_files))
        gt_top_k = set(ground_truth[:effective_k])

        pr_cov = coverage_at_k(pr_ordering, gt_top_k, effective_k)
        pr_prec = precision_at_k(pr_ordering, gt_top_k, effective_k)

        alpha_cov = coverage_at_k(alpha_ordering, gt_top_k, effective_k)
        alpha_prec = precision_at_k(alpha_ordering, gt_top_k, effective_k)

        rand_covs = []
        rand_precs = []
        for _ in range(random_trials):
            rand_ord = random_ordering(graph_files)
            rand_covs.append(coverage_at_k(rand_ord, gt_top_k, effective_k))
            rand_precs.append(precision_at_k(rand_ord, gt_top_k, effective_k))

        result.measurements.append({
            "k": k,
            "effective_k": effective_k,
            "pagerank": {"coverage": round(pr_cov, 4), "precision": round(pr_prec, 4)},
            "alphabetical": {"coverage": round(alpha_cov, 4), "precision": round(alpha_prec, 4)},
            "random": {
                "coverage_mean": round(sum(rand_covs) / len(rand_covs), 4),
                "precision_mean": round(sum(rand_precs) / len(rand_precs), 4),
                "trials": random_trials,
            },
        })

    # Print comparison table
    rows = []
    for m in result.measurements:
        rows.append([
            str(m["k"]),
            f'{m["pagerank"]["coverage"]:.1%}',
            f'{m["alphabetical"]["coverage"]:.1%}',
            f'{m["random"]["coverage_mean"]:.1%}',
            f'{m["pagerank"]["precision"]:.1%}',
            f'{m["random"]["precision_mean"]:.1%}',
        ])

    print_table(
        "PageRank vs Baselines: Coverage & Precision at k",
        ["k", "PR Coverage", "Alpha Coverage", "Rand Coverage", "PR Precision", "Rand Precision"],
        rows,
    )

    # Qualitative top-10 lists
    console.print("\n[bold]Top 10 by Dependent Count (Ground Truth):[/bold]")
    for i, f in enumerate(ground_truth[:10], 1):
        try:
            dep_count = len(graph.get_dependents(f))
        except Exception:
            dep_count = 0
        console.print(f"  {i:2d}. {Path(f).name} ({dep_count} dependents)")

    console.print("\n[bold]Top 10 by PageRank:[/bold]")
    ranked = graph.get_top_ranked_files(10)
    for i, (path, score) in enumerate(ranked, 1):
        console.print(f"  {i:2d}. {Path(path).name} (score: {score:.4f})")

    return result


def main():
    parser = create_arg_parser("Benchmark D: PageRank vs Random Ordering")
    parser.add_argument("--top-k", type=str, default="5,10,20,50")
    parser.add_argument("--random-trials", type=int, default=DEFAULT_RANDOM_TRIALS)
    args = parser.parse_args()

    top_k_values = [int(k) for k in args.top_k.split(",")]
    result = run_benchmark(args.repo, top_k_values, args.random_trials)
    if args.output:
        result.save(Path(args.output))
        console.print(f"Results saved to {args.output}")
    console.print("[bold green]Benchmark complete.[/bold green]")


if __name__ == "__main__":
    main()
