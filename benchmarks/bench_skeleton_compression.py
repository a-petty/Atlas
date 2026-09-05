"""Benchmark B: Skeleton Compression Ratio.

Measures the token savings from Atlas's skeleton generation (function/class
signatures with bodies replaced by '...') across all files in a repository.

Skeleton compression is currently Python-only. Non-Python files return the
original source unchanged (compression ratio = 1.0).
"""

from pathlib import Path
from typing import Dict, Optional
from datetime import datetime, timezone
from collections import defaultdict

from benchmarks.common import (
    create_arg_parser, init_graph, get_encoder, count_tokens,
    BenchmarkResult, print_table, console,
)
from atlas.semantic_engine import create_skeleton_from_source

SIZE_BUCKETS = [
    ("tiny", 0, 50),
    ("small", 50, 150),
    ("medium", 150, 500),
    ("large", 500, float("inf")),
]

EXT_TO_LANG = {
    ".py": "Python",
    ".pyi": "Python",
    ".rs": "Rust",
    ".js": "JavaScript",
    ".jsx": "JavaScript",
    ".ts": "TypeScript",
    ".tsx": "TypeScript",
    ".go": "Go",
    ".java": "Java",
}


def classify_size(line_count: int) -> str:
    for name, lo, hi in SIZE_BUCKETS:
        if lo <= line_count < hi:
            return name
    return "large"


def measure_file(file_path: Path, encoder) -> Optional[Dict]:
    """Measure skeleton compression for a single file."""
    try:
        source = file_path.read_text()
    except Exception:
        return None

    ext = file_path.suffix.lower()
    lang_name = EXT_TO_LANG.get(ext, "Unknown")
    lang_ext = ext.lstrip(".")

    line_count = source.count("\n") + 1
    original_tokens = count_tokens(source, encoder)

    if original_tokens == 0:
        return None

    try:
        skeleton = create_skeleton_from_source(source, lang_ext)
    except Exception:
        return None

    skeleton_tokens = count_tokens(skeleton, encoder)
    compression_ratio = skeleton_tokens / original_tokens

    return {
        "file": str(file_path),
        "language": lang_name,
        "line_count": line_count,
        "size_bucket": classify_size(line_count),
        "original_tokens": original_tokens,
        "skeleton_tokens": skeleton_tokens,
        "compression_ratio": round(compression_ratio, 4),
        "tokens_saved": original_tokens - skeleton_tokens,
        "has_real_compression": lang_name == "Python",
    }


def run_benchmark(repo_path: str) -> BenchmarkResult:
    """Run skeleton compression benchmark on all files in the repo."""
    graph, files = init_graph(repo_path)
    encoder = get_encoder()

    result = BenchmarkResult(
        benchmark_name="skeleton_compression",
        repo_path=str(Path(repo_path).resolve()),
        file_count=len(files),
        timestamp=datetime.now(timezone.utc).isoformat(),
    )

    console.print(f"[bold]Measuring skeleton compression for {len(files)} files...[/bold]\n")

    measurements = []
    for f in files:
        m = measure_file(Path(f), encoder)
        if m is not None:
            measurements.append(m)

    result.measurements = measurements

    # --- Aggregate by language ---
    by_lang = defaultdict(list)
    for m in measurements:
        by_lang[m["language"]].append(m)

    lang_rows = []
    for lang, items in sorted(by_lang.items()):
        ratios = [m["compression_ratio"] for m in items]
        avg_ratio = sum(ratios) / len(ratios)
        total_orig = sum(m["original_tokens"] for m in items)
        total_skel = sum(m["skeleton_tokens"] for m in items)
        total_saved = total_orig - total_skel
        real = items[0]["has_real_compression"]
        lang_rows.append([
            lang,
            str(len(items)),
            f"{avg_ratio:.3f}",
            f"{total_orig:,}",
            f"{total_skel:,}",
            f"{total_saved:,}",
            "Yes" if real else "No (passthrough)",
        ])

    print_table(
        "Skeleton Compression by Language",
        ["Language", "Files", "Avg Ratio", "Orig Tokens", "Skel Tokens", "Saved", "Real Compression"],
        lang_rows,
    )

    # --- Aggregate by size bucket (Python only) ---
    python_items = [m for m in measurements if m["has_real_compression"]]
    if python_items:
        by_size = defaultdict(list)
        for m in python_items:
            by_size[m["size_bucket"]].append(m)

        size_rows = []
        for bucket_name, _, _ in SIZE_BUCKETS:
            items = by_size.get(bucket_name, [])
            if not items:
                continue
            ratios = [m["compression_ratio"] for m in items]
            avg_ratio = sum(ratios) / len(ratios)
            size_rows.append([
                bucket_name,
                str(len(items)),
                f"{avg_ratio:.3f}",
                f"{min(ratios):.3f}",
                f"{max(ratios):.3f}",
            ])

        if size_rows:
            print_table(
                "Python Skeleton Compression by File Size",
                ["Size Bucket", "Files", "Avg Ratio", "Min Ratio", "Max Ratio"],
                size_rows,
            )

    # --- Summary stats ---
    if python_items:
        py_ratios = [m["compression_ratio"] for m in python_items]
        py_total_orig = sum(m["original_tokens"] for m in python_items)
        py_total_saved = sum(m["tokens_saved"] for m in python_items)
        console.print(
            f"\n[bold]Python summary:[/bold] {len(python_items)} files, "
            f"avg ratio {sum(py_ratios)/len(py_ratios):.3f}, "
            f"{py_total_saved:,} tokens saved out of {py_total_orig:,} "
            f"({py_total_saved/py_total_orig*100:.1f}% reduction)"
        )

    result.metadata = {
        "languages_found": sorted(by_lang.keys()),
        "python_file_count": len(python_items),
        "note": "Skeleton compression is Python-only. Non-Python files return source unchanged (ratio=1.0).",
    }

    return result


def main():
    parser = create_arg_parser("Benchmark B: Skeleton Compression Ratio")
    args = parser.parse_args()
    result = run_benchmark(args.repo)
    if args.output:
        result.save(Path(args.output))
        console.print(f"Results saved to {args.output}")
    console.print("[bold green]Benchmark complete.[/bold green]")


if __name__ == "__main__":
    main()
