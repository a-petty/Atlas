"""Benchmark E: ContextBench Evaluation.

Evaluates Atlas's context retrieval against ContextBench (arxiv 2602.05892),
a dataset of 1,136 human-annotated coding tasks from 66 repos across 8 languages.
Each task has a problem statement (GitHub issue) and gold context (exact files/lines
needed to resolve it). Measures how well Atlas's assemble_context retrieves the
right code.

Requires: pip install datasets
"""

import json
import re
import subprocess
import random
import logging
from pathlib import Path
from typing import List, Dict, Any, Optional, Set, Tuple
from dataclasses import dataclass, field, asdict
from datetime import datetime, timezone
from collections import defaultdict

from benchmarks.common import (
    init_graph, Timer, BenchmarkResult, print_table, console, IGNORED_DIRS,
)

logger = logging.getLogger(__name__)

DEFAULT_SAMPLE_SIZE = 10
DEFAULT_TOKEN_BUDGET = 32_000
DEFAULT_REPO_CACHE = "/tmp/contextbench_repos"


# ---------------------------------------------------------------------------
# Data structures
# ---------------------------------------------------------------------------

@dataclass
class GoldBlock:
    """A single gold context block from the dataset."""
    file: str
    start_line: int
    end_line: int
    content: str


@dataclass
class RetrievedFile:
    """A file retrieved by Atlas."""
    file: str   # relative path (normalized)
    mode: str   # "FULL CONTENT" or "SKELETON"
    retained_line_ranges: List[Tuple[int, int]] = field(default_factory=list)


@dataclass
class TaskResult:
    """Result for a single ContextBench task."""
    instance_id: str
    repo: str
    language: str
    # File-level metrics
    gold_files: List[str]
    retrieved_files: List[str]
    file_recall: float
    file_precision: float
    file_f1: float
    # Delivered-source block coverage
    gold_block_count: int
    blocks_fully_covered: int
    blocks_partially_covered: int
    blocks_missed: int
    block_recall_full: float
    block_recall_any: float
    # Timing
    graph_build_ms: float
    context_assembly_ms: float
    # Error info
    error: Optional[str] = None


# ---------------------------------------------------------------------------
# Dataset loading
# ---------------------------------------------------------------------------

def load_contextbench(
    subset: str = "default",
    language: Optional[str] = None,
    sample: Optional[int] = None,
    seed: int = 42,
) -> List[Dict]:
    """Load ContextBench tasks from HuggingFace.

    Args:
        subset: "default" for full 1,136 tasks, "verified" for 500-task curated subset.
        language: Filter to a single language (e.g., "python"). Case-insensitive.
        sample: If set, randomly sample this many tasks.
        seed: Random seed for reproducible sampling.
    """
    try:
        from datasets import load_dataset as hf_load
    except ImportError:
        console.print(
            "[bold red]ERROR:[/bold red] 'datasets' package not installed. "
            "Install with: pip install datasets"
        )
        raise SystemExit(1)

    dataset_name = "Contextbench/ContextBench"
    config_name = "contextbench_verified" if subset == "verified" else "default"

    console.print(f"Loading ContextBench dataset (subset={subset})...")
    ds = hf_load(dataset_name, config_name, split="train")
    tasks = list(ds)
    console.print(f"  Loaded {len(tasks)} tasks")

    if language:
        lang_lower = language.lower()
        tasks = [t for t in tasks if t.get("language", "").lower() == lang_lower]
        console.print(f"  Filtered to language={language}: {len(tasks)} tasks")

    if sample and sample < len(tasks):
        rng = random.Random(seed)
        tasks = rng.sample(tasks, sample)
        console.print(f"  Sampled {sample} tasks (seed={seed})")

    console.print(f"  Total tasks to evaluate: {len(tasks)}")
    return tasks


# ---------------------------------------------------------------------------
# Repo management
# ---------------------------------------------------------------------------

def ensure_repo(
    repo_name: str,
    repo_url: str,
    base_commit: str,
    cache_dir: str,
) -> Optional[Path]:
    """Clone (if needed) and checkout a repo at a specific commit.

    Uses bare-clone + git-worktree for disk efficiency when the same repo
    appears at multiple commits.

    Returns path to the worktree directory, or None on failure.
    """
    cache_path = Path(cache_dir)
    parts = repo_name.split("/", 1)
    if len(parts) != 2:
        logger.error(f"Invalid repo name format: {repo_name}")
        return None
    org, name = parts
    bare_dir = cache_path / org / f"{name}.git"
    worktree_dir = cache_path / org / name / "worktrees" / base_commit[:10]

    # Reuse existing worktree if it's at the right commit
    if worktree_dir.exists():
        try:
            result = subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=worktree_dir, capture_output=True, text=True, timeout=10,
            )
            if result.returncode == 0 and result.stdout.strip().startswith(base_commit[:10]):
                return worktree_dir
        except Exception:
            pass

    # Clone bare repo if needed
    if not bare_dir.exists():
        console.print(f"  Cloning {repo_name}...")
        bare_dir.parent.mkdir(parents=True, exist_ok=True)
        result = subprocess.run(
            ["git", "clone", "--bare", repo_url, str(bare_dir)],
            capture_output=True, text=True, timeout=600,
        )
        if result.returncode != 0:
            logger.error(f"Failed to clone {repo_name}: {result.stderr}")
            return None

    # Create worktree at the desired commit
    worktree_dir.parent.mkdir(parents=True, exist_ok=True)

    # Clean up stale worktree entry if present
    subprocess.run(
        ["git", "worktree", "remove", "--force", str(worktree_dir)],
        cwd=bare_dir, capture_output=True, text=True, timeout=30,
    )

    result = subprocess.run(
        ["git", "worktree", "add", "--detach", str(worktree_dir), base_commit],
        cwd=bare_dir, capture_output=True, text=True, timeout=120,
    )
    if result.returncode != 0:
        # Commit might not exist in bare clone — try fetching
        console.print(f"  Fetching {base_commit[:10]}...")
        subprocess.run(
            ["git", "fetch", "origin", base_commit],
            cwd=bare_dir, capture_output=True, text=True, timeout=300,
        )
        result = subprocess.run(
            ["git", "worktree", "add", "--detach", str(worktree_dir), base_commit],
            cwd=bare_dir, capture_output=True, text=True, timeout=120,
        )
        if result.returncode != 0:
            logger.error(
                f"Failed to checkout {base_commit[:10]} in {repo_name}: {result.stderr}"
            )
            return None

    return worktree_dir


# ---------------------------------------------------------------------------
# Parsing
# ---------------------------------------------------------------------------

def parse_gold_context(gold_context_raw) -> List[GoldBlock]:
    """Parse gold_context from the dataset.

    Handles both JSON strings and pre-parsed lists.
    """
    if isinstance(gold_context_raw, str):
        try:
            blocks_raw = json.loads(gold_context_raw)
        except (json.JSONDecodeError, TypeError):
            return []
    elif isinstance(gold_context_raw, list):
        blocks_raw = gold_context_raw
    else:
        return []

    blocks = []
    for b in blocks_raw:
        if not isinstance(b, dict):
            continue
        blocks.append(GoldBlock(
            file=b.get("file", ""),
            start_line=b.get("start_line", 0),
            end_line=b.get("end_line", 0),
            content=b.get("content", ""),
        ))
    return blocks


_FILE_PATTERN = re.compile(r"^# (.+?) \((FULL CONTENT|SKELETON)\)", re.MULTILINE)


def parse_atlas_retrieved_files(
    context_str: str, repo_root: Path
) -> List[RetrievedFile]:
    """Parse assemble_context() output to extract retrieved files.

    Normalizes absolute paths to repo-relative paths for comparison
    with ContextBench gold contexts.
    """
    matches = _FILE_PATTERN.findall(context_str)
    repo_root_str = str(repo_root.resolve())

    retrieved = []
    seen: Set[str] = set()

    for abs_path, mode in matches:
        # Normalize to relative path
        rel_path = abs_path
        if abs_path.startswith(repo_root_str):
            rel_path = abs_path[len(repo_root_str):].lstrip("/")

        if rel_path not in seen:
            retrieved.append(RetrievedFile(file=rel_path, mode=mode))
            seen.add(rel_path)

    return retrieved


# ---------------------------------------------------------------------------
# Metrics
# ---------------------------------------------------------------------------

def compute_file_metrics(
    gold_files: Set[str], retrieved_files: Set[str]
) -> Tuple[float, float, float]:
    """Compute file-level recall, precision, F1."""
    if not gold_files:
        return (1.0, 1.0, 1.0) if not retrieved_files else (1.0, 0.0, 0.0)

    tp = len(gold_files & retrieved_files)
    recall = tp / len(gold_files)
    precision = tp / len(retrieved_files) if retrieved_files else 0.0
    f1 = (2 * precision * recall / (precision + recall)) if (precision + recall) > 0 else 0.0
    return (recall, precision, f1)


def compute_block_metrics(
    gold_blocks: List[GoldBlock], retrieved: List[RetrievedFile]
) -> Tuple[int, int, int]:
    """Compatibility metric: only explicitly recorded retained whole lines count.

    Labels alone cannot prove source coverage. New evaluations use
    retained_span_metrics on the structured delivery result, including bytes.
    """
    from benchmarks.bench_retrieval import merged_ranges
    spans = defaultdict(list)
    for item in retrieved:
        spans[item.file].extend((start, end + 1) for start, end in item.retained_line_ranges)
    spans = {file: merged_ranges(ranges) for file, ranges in spans.items()}
    full = partial = missed = 0
    for block in gold_blocks:
        amount = sum(max(0, min(block.end_line + 1, end) - max(block.start_line, start))
                     for start, end in spans.get(block.file, []))
        if amount and amount == block.end_line + 1 - block.start_line:
            full += 1
        elif amount:
            partial += 1
        else:
            missed += 1
    return full, partial, missed


# ---------------------------------------------------------------------------
# Single task evaluation
# ---------------------------------------------------------------------------

def evaluate_task(
    task: Dict,
    cache_dir: str,
    token_budget: int = DEFAULT_TOKEN_BUDGET,
) -> TaskResult:
    """Run Atlas on a single ContextBench task and compute metrics."""
    instance_id = task["instance_id"]
    repo_name = task["repo"]
    language = task.get("language", "unknown")

    # Parse gold context
    gold_blocks = parse_gold_context(task.get("gold_context", "[]"))
    gold_files = sorted(set(b.file for b in gold_blocks))

    def make_error(msg: str) -> TaskResult:
        return TaskResult(
            instance_id=instance_id, repo=repo_name, language=language,
            gold_files=gold_files, retrieved_files=[],
            file_recall=0.0, file_precision=0.0, file_f1=0.0,
            gold_block_count=len(gold_blocks),
            blocks_fully_covered=0, blocks_partially_covered=0,
            blocks_missed=len(gold_blocks),
            block_recall_full=0.0, block_recall_any=0.0,
            graph_build_ms=0.0, context_assembly_ms=0.0,
            error=msg,
        )

    # Step 1: Ensure repo is available
    repo_url = task.get("repo_url", f"https://github.com/{repo_name}.git")
    base_commit = task["base_commit"]
    repo_path = ensure_repo(repo_name, repo_url, base_commit, cache_dir)
    if repo_path is None:
        return make_error(f"Failed to clone/checkout {repo_name}@{base_commit[:10]}")

    try:
        # Step 2: Build graph
        with Timer() as build_timer:
            graph, files = init_graph(str(repo_path))

        # Step 3: Init embeddings + context manager
        from atlas.embeddings import EmbeddingManager
        from atlas.context import ContextManager

        emb = EmbeddingManager(repo_graph=graph)
        ctx = ContextManager(graph, emb, max_tokens=token_budget)

        # Step 4: Run context assembly
        query = task.get("problem_statement", "")
        if not query:
            return make_error("No problem_statement in task")

        with Timer() as ctx_timer:
            context_result = ctx.assemble_context_result(query, files_in_scope=[])

        # Step 5: Parse output
        retrieved = [RetrievedFile(file=str(Path(c["file"]).relative_to(repo_path)), mode=c["mode"])
                     for c in context_result.chunks]
        retrieved_files = sorted(set(r.file for r in retrieved))

        # Step 6: Compute metrics
        gold_file_set = set(gold_files)
        retrieved_file_set = set(retrieved_files)

        recall, precision, f1 = compute_file_metrics(gold_file_set, retrieved_file_set)
        from benchmarks.bench_retrieval import retained_span_metrics
        coverage = retained_span_metrics(gold_blocks, context_result, repo_path)
        fully = coverage["blocks_fully_covered"]
        partially = coverage["blocks_partially_covered"]
        missed = coverage["blocks_missed"]

        total_blocks = len(gold_blocks) if gold_blocks else 1
        block_recall_full = fully / total_blocks
        block_recall_any = (fully + partially) / total_blocks

        return TaskResult(
            instance_id=instance_id, repo=repo_name, language=language,
            gold_files=gold_files, retrieved_files=retrieved_files,
            file_recall=recall, file_precision=precision, file_f1=f1,
            gold_block_count=len(gold_blocks),
            blocks_fully_covered=fully,
            blocks_partially_covered=partially,
            blocks_missed=missed,
            block_recall_full=block_recall_full,
            block_recall_any=block_recall_any,
            graph_build_ms=build_timer.elapsed * 1000,
            context_assembly_ms=ctx_timer.elapsed * 1000,
        )

    except Exception as e:
        logger.exception(f"Error evaluating {instance_id}")
        return make_error(str(e))


# ---------------------------------------------------------------------------
# Aggregation and reporting
# ---------------------------------------------------------------------------

def aggregate_results(results: List[TaskResult]) -> Dict[str, Any]:
    """Compute aggregate metrics across all task results."""
    successful = [r for r in results if r.error is None]
    by_lang: Dict[str, List[TaskResult]] = defaultdict(list)
    for r in results:
        by_lang[r.language.lower()].append(r)

    def summarize(task_results: List[TaskResult]) -> Dict[str, Any]:
        if not task_results:
            return {}
        n = len(task_results)
        return {
            "count": n,
            "file_recall_mean": round(sum(r.file_recall for r in task_results) / n, 4),
            "file_precision_mean": round(sum(r.file_precision for r in task_results) / n, 4),
            "file_f1_mean": round(sum(r.file_f1 for r in task_results) / n, 4),
            "block_recall_full_mean": round(sum(r.block_recall_full for r in task_results) / n, 4),
            "block_recall_any_mean": round(sum(r.block_recall_any for r in task_results) / n, 4),
            "avg_graph_build_ms": round(sum(r.graph_build_ms for r in task_results) / n, 1),
            "avg_context_assembly_ms": round(sum(r.context_assembly_ms for r in task_results) / n, 1),
            "avg_gold_files": round(sum(len(r.gold_files) for r in task_results) / n, 1),
            "avg_retrieved_files": round(sum(len(r.retrieved_files) for r in task_results) / n, 1),
        }

    return {
        "overall": summarize(results),
        "by_language": {lang: summarize(tasks) for lang, tasks in sorted(by_lang.items())},
        "total_tasks": len(results),
        "successful_tasks": len(successful),
        "failed_tasks": len(results) - len(successful),
    }


def print_results(aggregated: Dict[str, Any], results: List[TaskResult]) -> None:
    """Print rich tables summarizing the evaluation."""
    overall = aggregated["overall"]
    if overall:
        console.print(f"\n[bold]Overall ({overall['count']} tasks):[/bold]")
        console.print(f"  File Recall:    {overall['file_recall_mean']:.1%}")
        console.print(f"  File Precision: {overall['file_precision_mean']:.1%}")
        console.print(f"  File F1:        {overall['file_f1_mean']:.1%}")
        console.print(f"  Block Recall (full):   {overall['block_recall_full_mean']:.1%}")
        console.print(f"  Block Recall (any):    {overall['block_recall_any_mean']:.1%}")

    # Per-language table
    lang_rows = []
    for lang, stats in sorted(aggregated["by_language"].items()):
        lang_rows.append([
            lang,
            str(stats["count"]),
            f"{stats['file_recall_mean']:.1%}",
            f"{stats['file_precision_mean']:.1%}",
            f"{stats['file_f1_mean']:.1%}",
            f"{stats['block_recall_full_mean']:.1%}",
            f"{stats['block_recall_any_mean']:.1%}",
        ])

    if lang_rows:
        print_table(
            "ContextBench Results by Language",
            ["Language", "Tasks", "File Recall", "File Prec", "File F1",
             "Block Recall (full)", "Block Recall (any)"],
            lang_rows,
        )

    # Per-task detail table
    successful = sorted(
        [r for r in results if r.error is None],
        key=lambda r: r.file_f1,
        reverse=True,
    )
    detail_rows = []
    for r in successful[:20]:
        detail_rows.append([
            r.instance_id[:50],
            r.language,
            str(len(r.gold_files)),
            str(len(r.retrieved_files)),
            f"{r.file_recall:.0%}",
            f"{r.file_f1:.0%}",
            f"{r.context_assembly_ms:.0f}ms",
        ])

    if detail_rows:
        print_table(
            "Per-Task Detail (top 20 by F1)",
            ["Instance ID", "Lang", "Gold", "Retrieved", "Recall", "F1", "Time"],
            detail_rows,
        )

    # Errors
    errors = [r for r in results if r.error is not None]
    if errors:
        console.print(f"\n[bold red]{len(errors)} tasks failed:[/bold red]")
        for r in errors[:10]:
            console.print(f"  {r.instance_id}: {r.error}")
        if len(errors) > 10:
            console.print(f"  ... and {len(errors) - 10} more")


# ---------------------------------------------------------------------------
# Main orchestration
# ---------------------------------------------------------------------------

def run_benchmark(
    subset: str = "default",
    language: Optional[str] = None,
    sample: Optional[int] = DEFAULT_SAMPLE_SIZE,
    token_budget: int = DEFAULT_TOKEN_BUDGET,
    repo_cache: str = DEFAULT_REPO_CACHE,
    seed: int = 42,
) -> BenchmarkResult:
    """Run the full ContextBench evaluation."""
    tasks = load_contextbench(
        subset=subset, language=language, sample=sample, seed=seed,
    )

    if not tasks:
        console.print("[bold red]No tasks to evaluate.[/bold red]")
        return BenchmarkResult(
            benchmark_name="contextbench",
            repo_path="(multi-repo)",
            file_count=0,
            timestamp=datetime.now(timezone.utc).isoformat(),
        )

    results: List[TaskResult] = []

    for i, task in enumerate(tasks, 1):
        console.print(
            f"\n[bold]Task {i}/{len(tasks)}:[/bold] "
            f"{task['instance_id']} ({task.get('language', '?')}, {task['repo']})"
        )

        result = evaluate_task(task, cache_dir=repo_cache, token_budget=token_budget)

        if result.error:
            console.print(f"  [red]ERROR: {result.error}[/red]")
        else:
            console.print(
                f"  File recall={result.file_recall:.0%} "
                f"precision={result.file_precision:.0%} "
                f"F1={result.file_f1:.0%} "
                f"| Gold={len(result.gold_files)} Retrieved={len(result.retrieved_files)} "
                f"| {result.context_assembly_ms:.0f}ms"
            )

        results.append(result)

    # Aggregate and print
    aggregated = aggregate_results(results)
    print_results(aggregated, results)

    return BenchmarkResult(
        benchmark_name="contextbench",
        repo_path="(multi-repo)",
        file_count=len(tasks),
        timestamp=datetime.now(timezone.utc).isoformat(),
        measurements=[asdict(r) for r in results],
        metadata={
            "subset": subset,
            "language_filter": language,
            "sample_size": sample,
            "token_budget": token_budget,
            "seed": seed,
            **aggregated,
        },
    )


def main():
    import argparse

    parser = argparse.ArgumentParser(
        description="Benchmark E: ContextBench Evaluation",
        epilog="Example: python -m benchmarks.bench_contextbench --sample 5 --language python",
    )
    parser.add_argument(
        "--sample", type=int, default=DEFAULT_SAMPLE_SIZE,
        help=f"Number of tasks to sample (default: {DEFAULT_SAMPLE_SIZE}, 0=all)",
    )
    parser.add_argument(
        "--language", type=str, default=None,
        help="Filter to a single language (e.g., python, javascript)",
    )
    parser.add_argument(
        "--subset", type=str, default="default",
        choices=["default", "verified"],
        help="Dataset subset (default: 'default')",
    )
    parser.add_argument(
        "--token-budget", type=int, default=DEFAULT_TOKEN_BUDGET,
        help=f"Token budget for assemble_context (default: {DEFAULT_TOKEN_BUDGET})",
    )
    parser.add_argument(
        "--repo-cache", type=str, default=DEFAULT_REPO_CACHE,
        help=f"Directory to cache cloned repos (default: {DEFAULT_REPO_CACHE})",
    )
    parser.add_argument(
        "--seed", type=int, default=42,
        help="Random seed for sampling (default: 42)",
    )
    parser.add_argument(
        "--output", type=str, default=None,
        help="Path to write JSON results",
    )
    args = parser.parse_args()

    sample = args.sample if args.sample > 0 else None

    result = run_benchmark(
        subset=args.subset,
        language=args.language,
        sample=sample,
        token_budget=args.token_budget,
        repo_cache=args.repo_cache,
        seed=args.seed,
    )

    if args.output:
        result.save(Path(args.output))
        console.print(f"\nResults saved to {args.output}")

    console.print("\n[bold green]ContextBench evaluation complete.[/bold green]")


if __name__ == "__main__":
    main()
