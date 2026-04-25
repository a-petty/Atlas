"""End-to-end smoke tests for Phase 3b auto-detection.

Verifies that constructing `RepoGraph` with no `language` / `languages`
argument auto-detects resolver groups from disk, and that the detection
metadata flows through to `GraphStatistics` for the MCP `atlas_status`
tool to render.

Policy under test (locked in REMEDIATION_PLAN.md):
- Present-is-enough threshold (>=1 file)
- Zero-detection registers no resolvers (not a Python fallback)
- Languages without a resolver surface in `unsupported_language_counts`
"""

import os
import tempfile
import warnings

import pytest

from atlas import semantic_engine


def _make_repo(files):
    """Create a temp directory with the given relative file paths (all empty)."""
    tmp = tempfile.mkdtemp()
    for rel in files:
        full = os.path.join(tmp, rel)
        parent = os.path.dirname(full)
        if parent:
            os.makedirs(parent, exist_ok=True)
        open(full, "w").close()
    return tmp


def _stats_no_warnings(project_root):
    """Construct a graph with no language argument and confirm no warnings."""
    with warnings.catch_warnings(record=True) as ws:
        warnings.simplefilter("always")
        g = semantic_engine.RepoGraph(project_root)
        deps = [w for w in ws if issubclass(w.category, DeprecationWarning)]
        assert deps == [], f"auto-detect path must not emit deprecation warnings, got {deps}"
    return g.get_statistics()


def test_python_only_repo_registers_python_resolver():
    root = _make_repo(["a.py", "pkg/b.py"])
    stats = _stats_no_warnings(root)
    assert stats.registered_resolvers == ["python"]
    assert dict(stats.unsupported_language_counts) == {}
    assert dict(stats.file_counts_by_language) == {"python": 2}


def test_mixed_python_typescript_registers_both():
    root = _make_repo(["api/main.py", "frontend/app.ts", "frontend/page.tsx"])
    stats = _stats_no_warnings(root)
    # `registered_resolvers` is sourced from the resolver `BTreeMap` (ordered
    # by `ResolverGroup`'s derived `Ord`: Python < JsTs), so the order is
    # stable but not driven by file counts. Detection's count-desc ordering
    # only matters for the input list passed to the constructor.
    assert stats.registered_resolvers == ["python", "javascript_typescript"]
    assert dict(stats.unsupported_language_counts) == {}
    counts = dict(stats.file_counts_by_language)
    assert counts.get("python") == 1
    assert counts.get("typescript") == 1
    assert counts.get("typescript_tsx") == 1


def test_go_only_repo_registers_no_resolvers_and_surfaces_gap():
    """The load-bearing test for the 'Atlas worse than grep' honesty story.

    A Go-only repo must not silently fall back to a Python resolver. It
    registers nothing, and the unsupported-language gap is visible to any
    caller of `get_statistics`.
    """
    root = _make_repo(["main.go", "pkg/util.go", "cmd/server.go"])
    stats = _stats_no_warnings(root)
    assert stats.registered_resolvers == []
    assert dict(stats.unsupported_language_counts) == {"go": 3}
    # No Python files → Python must not show up as having 0 files.
    assert "python" not in dict(stats.file_counts_by_language)
    # And import bookkeeping reflects the absence of resolvers.
    assert stats.attempted_imports == 0
    assert stats.failed_imports == 0


def test_explicit_languages_arg_skips_detection():
    """When the caller passes `languages=[...]` explicitly, detection metadata
    is not populated — the caller has opted out of auto-detection and owns
    the coverage picture themselves."""
    root = _make_repo(["a.py", "tool.go"])
    g = semantic_engine.RepoGraph(root, languages=["python"])
    stats = g.get_statistics()
    assert stats.registered_resolvers == ["python"]
    # Detection ran nowhere — both maps stay empty.
    assert dict(stats.file_counts_by_language) == {}
    assert dict(stats.unsupported_language_counts) == {}
