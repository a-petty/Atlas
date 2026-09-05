"""Run all Atlas benchmarks and save results.

Execution order: B (skeleton) -> C (latency) -> D (pagerank) -> A (token efficiency)
Fast benchmarks run first; token efficiency last since it initializes embeddings.
ContextBench (E) is opt-in via --include-contextbench since it requires network access.
"""

from pathlib import Path

from benchmarks.common import create_arg_parser, console


def main():
    parser = create_arg_parser("Run all Atlas benchmarks")
    parser.add_argument("--iterations", type=int, default=50,
                        help="Iterations for latency benchmark (default: 50)")
    parser.add_argument("--include-contextbench", action="store_true",
                        help="Include ContextBench evaluation (requires network + 'datasets' package)")
    parser.add_argument("--contextbench-sample", type=int, default=10,
                        help="Number of ContextBench tasks to sample (default: 10)")
    args = parser.parse_args()

    output_dir = Path(args.output) if args.output else None
    if output_dir:
        output_dir.mkdir(parents=True, exist_ok=True)

    from benchmarks.bench_skeleton_compression import run_benchmark as run_skeleton
    from benchmarks.bench_incremental_latency import run_benchmark as run_latency
    from benchmarks.bench_pagerank_ordering import run_benchmark as run_pagerank
    from benchmarks.bench_token_efficiency import run_benchmark as run_tokens

    console.rule("[bold]Benchmark B: Skeleton Compression[/bold]")
    r = run_skeleton(args.repo)
    if output_dir:
        r.save(output_dir / "skeleton_compression.json")

    console.rule("[bold]Benchmark C: Incremental Latency[/bold]")
    r = run_latency(args.repo, args.iterations)
    if output_dir:
        r.save(output_dir / "incremental_latency.json")

    console.rule("[bold]Benchmark D: PageRank Ordering[/bold]")
    r = run_pagerank(args.repo)
    if output_dir:
        r.save(output_dir / "pagerank_ordering.json")

    console.rule("[bold]Benchmark A: Token Efficiency[/bold]")
    r = run_tokens(args.repo)
    if output_dir:
        r.save(output_dir / "token_efficiency.json")

    if args.include_contextbench:
        console.rule("[bold]Benchmark E: ContextBench[/bold]")
        try:
            from benchmarks.bench_contextbench import run_benchmark as run_contextbench
            r = run_contextbench(sample=args.contextbench_sample)
            if output_dir:
                r.save(output_dir / "contextbench.json")
        except Exception as e:
            console.print(f"[bold red]ContextBench skipped: {e}[/bold red]")

    console.rule("[bold green]All benchmarks complete[/bold green]")
    if output_dir:
        console.print(f"Results saved to {output_dir}/")


if __name__ == "__main__":
    main()
