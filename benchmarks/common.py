"""Shared utilities for Atlas benchmarks."""

import json
import time
import argparse
from pathlib import Path
from dataclasses import dataclass, field, asdict
from typing import List, Dict, Any, Optional, Tuple

import numpy as np
import tiktoken
from rich.console import Console
from rich.table import Table

from atlas.semantic_engine import RepoGraph, scan_repository

IGNORED_DIRS = [
    "node_modules", "target", ".git", "__pycache__",
    "dist", "build", ".venv", "venv",
]

console = Console()

_encoder: Optional[tiktoken.Encoding] = None


def get_encoder() -> tiktoken.Encoding:
    """Get the cl100k_base tiktoken encoder (cached)."""
    global _encoder
    if _encoder is None:
        _encoder = tiktoken.get_encoding("cl100k_base")
    return _encoder


def count_tokens(text: str, encoder: Optional[tiktoken.Encoding] = None) -> int:
    """Count tokens using cl100k_base encoding."""
    if encoder is None:
        encoder = get_encoder()
    return len(encoder.encode(text))


@dataclass
class BenchmarkResult:
    """Container for benchmark results with JSON serialization."""
    benchmark_name: str
    repo_path: str
    file_count: int
    timestamp: str
    measurements: List[Dict[str, Any]] = field(default_factory=list)
    metadata: Dict[str, Any] = field(default_factory=dict)

    def to_json(self) -> str:
        return json.dumps(asdict(self), indent=2)

    def save(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(self.to_json())


class Timer:
    """Context manager for precise wall-clock timing."""

    def __init__(self):
        self.elapsed: float = 0.0
        self._start: float = 0.0

    def __enter__(self):
        self._start = time.perf_counter()
        return self

    def __exit__(self, *args):
        self.elapsed = time.perf_counter() - self._start


def init_graph(repo_path: str) -> Tuple[RepoGraph, List[str]]:
    """Initialize a RepoGraph for the given repository.

    Returns (graph, file_paths).
    """
    repo = str(Path(repo_path).resolve())
    files = scan_repository(repo, ignored_dirs=IGNORED_DIRS)
    graph = RepoGraph(repo, ignored_dirs=IGNORED_DIRS)
    graph.build_complete(files)
    graph.ensure_pagerank_up_to_date()
    return graph, files


def compute_percentiles(values: List[float]) -> Dict[str, float]:
    """Compute summary statistics from a list of values."""
    if not values:
        return {"mean": 0.0, "median": 0.0, "p50": 0.0, "p95": 0.0, "p99": 0.0, "min": 0.0, "max": 0.0}
    arr = np.array(values)
    return {
        "mean": float(np.mean(arr)),
        "median": float(np.median(arr)),
        "p50": float(np.percentile(arr, 50)),
        "p95": float(np.percentile(arr, 95)),
        "p99": float(np.percentile(arr, 99)),
        "min": float(np.min(arr)),
        "max": float(np.max(arr)),
    }


def print_table(title: str, columns: List[str], rows: List[List[Any]]) -> None:
    """Print a rich table to the console."""
    table = Table(title=title)
    for col in columns:
        table.add_column(col)
    for row in rows:
        table.add_row(*[str(v) for v in row])
    console.print(table)


def create_arg_parser(description: str) -> argparse.ArgumentParser:
    """Create a standard argument parser with --repo and --output flags."""
    parser = argparse.ArgumentParser(description=description)
    parser.add_argument(
        "--repo", type=str, default=".",
        help="Path to repository to benchmark (default: current directory)",
    )
    parser.add_argument(
        "--output", type=str, default=None,
        help="Path to write JSON results",
    )
    return parser
