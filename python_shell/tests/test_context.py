"""Tests for ContextManager: neighborhood BFS, model-aware budgets, adaptive params."""

import pytest
from dataclasses import dataclass
from pathlib import Path
from unittest.mock import MagicMock, patch


@dataclass
class FakeStats:
    """Mock of RepoGraph statistics."""
    node_count: int = 100
    edge_count: int = 300
    total_definitions: int = 500


class FakeRepoGraph:
    """Minimal mock of RepoGraph for testing neighborhood traversal."""

    def __init__(self, node_count: int = 100, edge_count: int = 300):
        # dep_map: {file_str: [(neighbor_str, edge_kind), ...]}
        self.dep_map = {}
        # dependent_map: {file_str: [(dependent_str, edge_kind), ...]}
        self.dependent_map = {}
        self._node_count = node_count
        self._edge_count = edge_count

    def add_edge(self, source: str, target: str, edge_kind: str):
        """Add a directed edge: source depends on target."""
        self.dep_map.setdefault(source, []).append((target, edge_kind))
        self.dependent_map.setdefault(target, []).append((source, edge_kind))

    def get_dependencies(self, path: str):
        return self.dep_map.get(path, [])

    def get_dependents(self, path: str):
        return self.dependent_map.get(path, [])

    def get_top_ranked_files(self, n):
        return []

    def generate_map(self, max_files=50):
        return ""

    def get_statistics(self):
        return FakeStats(node_count=self._node_count, edge_count=self._edge_count)


def _make_context_manager(graph):
    """Construct a ContextManager with mocked embedding manager and tiktoken."""
    with patch("atlas.context.tiktoken") as mock_tiktoken:
        mock_encoder = MagicMock()
        mock_encoder.encode.return_value = []
        mock_tiktoken.encoding_for_model.return_value = mock_encoder

        mock_embed = MagicMock()
        from atlas.context import ContextManager
        cm = ContextManager(graph, mock_embed, model="gpt-4", max_tokens=100000)
        return cm


class TestBugFix:
    """Verify the original bug (edge_kind not in neighborhood) is fixed."""

    def test_import_edge_dedup_uses_dep_path(self):
        """Import edges should not always be added — only if dep_path is new."""
        graph = FakeRepoGraph()
        # A -> B via Import, A -> B via SymbolUsage (same target, two edges)
        a = str(Path("/repo/a.py").resolve())
        b = str(Path("/repo/b.py").resolve())
        graph.add_edge(a, b, "Import")
        graph.add_edge(a, b, "SymbolUsage")

        cm = _make_context_manager(graph)
        result = cm._get_dependency_neighborhood(
            [Path("/repo/a.py")],
            already_processed=set(),
            max_hops=1,
            max_files=30,
        )

        # B should appear exactly once
        paths = [p for p, _ in result]
        assert paths.count(Path(b)) == 1


class TestMultiHopBFS:
    """Test multi-hop traversal depth and edge-type rules."""

    def _build_chain_graph(self):
        """Build a linear chain: A -> B -> C -> D via SymbolUsage."""
        graph = FakeRepoGraph()
        a = str(Path("/repo/a.py").resolve())
        b = str(Path("/repo/b.py").resolve())
        c = str(Path("/repo/c.py").resolve())
        d = str(Path("/repo/d.py").resolve())
        graph.add_edge(a, b, "SymbolUsage")
        graph.add_edge(b, c, "SymbolUsage")
        graph.add_edge(c, d, "SymbolUsage")
        return graph, a, b, c, d

    def test_single_hop(self):
        """With max_hops=1, only direct neighbors are returned."""
        graph, a, b, c, d = self._build_chain_graph()
        cm = _make_context_manager(graph)

        result = cm._get_dependency_neighborhood(
            [Path("/repo/a.py")],
            already_processed=set(),
            max_hops=1,
            max_files=30,
        )
        paths = {p for p, _ in result}

        assert Path(b) in paths
        assert Path(c) not in paths
        assert Path(d) not in paths

    def test_two_hop(self):
        """With max_hops=2, two levels of neighbors are returned."""
        graph, a, b, c, d = self._build_chain_graph()
        cm = _make_context_manager(graph)

        result = cm._get_dependency_neighborhood(
            [Path("/repo/a.py")],
            already_processed=set(),
            max_hops=2,
            max_files=30,
        )
        paths = {p for p, _ in result}

        assert Path(b) in paths
        assert Path(c) in paths
        assert Path(d) not in paths

    def test_three_hop(self):
        """With max_hops=3, three levels are returned."""
        graph, a, b, c, d = self._build_chain_graph()
        cm = _make_context_manager(graph)

        result = cm._get_dependency_neighborhood(
            [Path("/repo/a.py")],
            already_processed=set(),
            max_hops=3,
            max_files=30,
        )
        paths = {p for p, _ in result}

        assert Path(b) in paths
        assert Path(c) in paths
        assert Path(d) in paths


class TestImportEdgeCapping:
    """Import edges should only be followed for 1 hop."""

    def test_import_edges_capped_at_one_hop(self):
        """A chain via Import edges should only reach 1 hop deep."""
        graph = FakeRepoGraph()
        a = str(Path("/repo/a.py").resolve())
        b = str(Path("/repo/b.py").resolve())
        c = str(Path("/repo/c.py").resolve())
        graph.add_edge(a, b, "Import")
        graph.add_edge(b, c, "Import")

        cm = _make_context_manager(graph)
        result = cm._get_dependency_neighborhood(
            [Path("/repo/a.py")],
            already_processed=set(),
            max_hops=3,
            max_files=30,
        )
        paths = {p for p, _ in result}

        assert Path(b) in paths
        # C should NOT be reached — Import edges stop after 1 hop
        assert Path(c) not in paths

    def test_symbol_usage_continues_past_import_limit(self):
        """SymbolUsage edges should continue even when Import edges stop."""
        graph = FakeRepoGraph()
        a = str(Path("/repo/a.py").resolve())
        b = str(Path("/repo/b.py").resolve())
        c = str(Path("/repo/c.py").resolve())
        graph.add_edge(a, b, "SymbolUsage")
        graph.add_edge(b, c, "SymbolUsage")

        cm = _make_context_manager(graph)
        result = cm._get_dependency_neighborhood(
            [Path("/repo/a.py")],
            already_processed=set(),
            max_hops=2,
            max_files=30,
        )
        paths = {p for p, _ in result}

        assert Path(b) in paths
        assert Path(c) in paths


class TestDistanceDecay:
    """Verify distance-decay weighting."""

    def test_hop_weights(self):
        """Hop 1 = 1.0, hop 2 = 0.5."""
        graph = FakeRepoGraph()
        a = str(Path("/repo/a.py").resolve())
        b = str(Path("/repo/b.py").resolve())
        c = str(Path("/repo/c.py").resolve())
        graph.add_edge(a, b, "SymbolUsage")
        graph.add_edge(b, c, "SymbolUsage")

        cm = _make_context_manager(graph)
        result = cm._get_dependency_neighborhood(
            [Path("/repo/a.py")],
            already_processed=set(),
            max_hops=2,
            max_files=30,
        )

        weight_map = {p: w for p, w in result}
        assert weight_map[Path(b)] == 1.0
        assert weight_map[Path(c)] == 0.5

    def test_hop3_weight(self):
        """Hop 3 = 0.25."""
        graph = FakeRepoGraph()
        a = str(Path("/repo/a.py").resolve())
        b = str(Path("/repo/b.py").resolve())
        c = str(Path("/repo/c.py").resolve())
        d = str(Path("/repo/d.py").resolve())
        graph.add_edge(a, b, "SymbolUsage")
        graph.add_edge(b, c, "SymbolUsage")
        graph.add_edge(c, d, "SymbolUsage")

        cm = _make_context_manager(graph)
        result = cm._get_dependency_neighborhood(
            [Path("/repo/a.py")],
            already_processed=set(),
            max_hops=3,
            max_files=30,
        )

        weight_map = {p: w for p, w in result}
        assert weight_map[Path(d)] == 0.25


class TestMaxFilesTruncation:
    """Verify max_files cap works."""

    def test_max_files_limits_results(self):
        """Should return at most max_files results."""
        graph = FakeRepoGraph()
        a = str(Path("/repo/a.py").resolve())
        # Create 10 direct deps
        for i in range(10):
            dep = str(Path(f"/repo/dep_{i}.py").resolve())
            graph.add_edge(a, dep, "SymbolUsage")

        cm = _make_context_manager(graph)
        result = cm._get_dependency_neighborhood(
            [Path("/repo/a.py")],
            already_processed=set(),
            max_hops=1,
            max_files=5,
        )

        assert len(result) == 5

    def test_already_processed_excluded(self):
        """Files in already_processed should not appear in results."""
        graph = FakeRepoGraph()
        a = str(Path("/repo/a.py").resolve())
        b = str(Path("/repo/b.py").resolve())
        c = str(Path("/repo/c.py").resolve())
        graph.add_edge(a, b, "SymbolUsage")
        graph.add_edge(a, c, "SymbolUsage")

        cm = _make_context_manager(graph)
        result = cm._get_dependency_neighborhood(
            [Path("/repo/a.py")],
            already_processed={Path(b)},
            max_hops=1,
            max_files=30,
        )
        paths = {p for p, _ in result}

        assert Path(b) not in paths
        assert Path(c) in paths


# ---------------------------------------------------------------------------
# Model-aware token budget tests
# ---------------------------------------------------------------------------

def _make_cm_with_stats(node_count=100, edge_count=300, model="gpt-4", max_tokens=None):
    """Build a ContextManager with configurable FakeRepoGraph stats."""
    graph = FakeRepoGraph(node_count=node_count, edge_count=edge_count)
    return _make_context_manager_ext(graph, model=model, max_tokens=max_tokens)


def _make_context_manager_ext(graph, model="gpt-4", max_tokens=None):
    """Construct a ContextManager with explicit model/max_tokens and mocked tiktoken."""
    with patch("atlas.context.tiktoken") as mock_tiktoken:
        mock_encoder = MagicMock()
        # Simple token counting: 1 token per 4 characters
        mock_encoder.encode = lambda text: [0] * (len(text) // 4)
        mock_encoder.decode = lambda tokens: "x" * len(tokens)
        mock_tiktoken.encoding_for_model.return_value = mock_encoder

        mock_embed = MagicMock()
        from atlas.context import ContextManager
        cm = ContextManager(graph, mock_embed, model=model, max_tokens=max_tokens)
        return cm


class TestModelContextWindows:
    """Tests for _resolve_max_tokens model-aware budget resolution."""

    def test_known_model_exact_match(self):
        from atlas.context import ContextManager
        result = ContextManager._resolve_max_tokens("deepseek-coder", None)
        assert result == int(128_000 * 0.60)

    def test_known_model_prefix_match(self):
        from atlas.context import ContextManager
        result = ContextManager._resolve_max_tokens("deepseek-coder:7b", None)
        assert result == int(128_000 * 0.60)

    def test_unknown_model_fallback(self):
        from atlas.context import ContextManager
        result = ContextManager._resolve_max_tokens("my-custom-model", None)
        assert result == int(100_000 * 0.60)

    def test_explicit_override(self):
        from atlas.context import ContextManager
        result = ContextManager._resolve_max_tokens("deepseek-coder", 50_000)
        assert result == 50_000

    def test_case_insensitive(self):
        from atlas.context import ContextManager
        result = ContextManager._resolve_max_tokens("Claude-Opus", None)
        assert result == int(200_000 * 0.60)


# ---------------------------------------------------------------------------
# Adaptive parameter tests
# ---------------------------------------------------------------------------

class TestAdaptiveParams:
    """Tests for _compute_adaptive_params adaptive context assembly."""

    def _get_params(self, node_count, edge_count, map_text="map", total_budget=60_000):
        cm = _make_cm_with_stats(node_count=node_count, edge_count=edge_count)
        return cm._compute_adaptive_params(total_budget, map_text)

    def test_tiny_repo(self):
        """10 files, 15 edges: high tier2_share, 3 anchors, hops=3 (sparse)."""
        params = self._get_params(10, 15)
        # tier2_share = max(0.40, min(0.75, 0.75 - 10/1500)) ≈ 0.743
        assert params.anchor_count == 3
        assert params.neighborhood_max_hops == 3  # density 1.5 < 3.0
        assert params.neighborhood_max_files == 10  # min clamp

    def test_medium_repo(self):
        """200 files, 600 edges: balanced split, 10 anchors, hops=2."""
        params = self._get_params(200, 600)
        assert params.anchor_count == 10  # 200//10 = 20, capped at 10
        assert params.neighborhood_max_hops == 2  # density 3.0 >= 3.0
        assert params.neighborhood_max_files == 40  # 200//5 = 40

    def test_large_repo(self):
        """1000 files, 4000 edges: tier2_share = 0.40 floor, 10 anchors."""
        params = self._get_params(1000, 4000)
        assert params.anchor_count == 10
        assert params.neighborhood_max_hops == 2  # density 4.0 >= 3.0
        assert params.neighborhood_max_files == 40  # 1000//5 = 200, capped at 40

    def test_sparse_repo_gets_deeper_hops(self):
        """100 files, 150 edges (density 1.5): hops=3."""
        params = self._get_params(100, 150)
        assert params.neighborhood_max_hops == 3

    def test_dense_repo_gets_shallow_hops(self):
        """100 files, 500 edges (density 5.0): hops=2."""
        params = self._get_params(100, 500)
        assert params.neighborhood_max_hops == 2

    def test_small_map_uses_actual_tokens(self):
        """A small map (500 tokens) should use actual cost, not 8% cap."""
        # "x" * 2000 → 500 tokens with our mock (len // 4)
        map_text = "x" * 2000
        params = self._get_params(100, 300, map_text=map_text, total_budget=60_000)
        assert params.tier1_tokens == 500  # actual, not 60000*0.08=4800

    def test_large_map_capped_at_8_percent(self):
        """A huge map should be capped at 8% of total budget."""
        # "x" * 80000 → 20000 tokens with our mock
        map_text = "x" * 80_000
        params = self._get_params(100, 300, map_text=map_text, total_budget=60_000)
        assert params.tier1_tokens == int(60_000 * 0.08)  # 4800, not 20000

    def test_tier_budgets_sum_to_total(self):
        """tier1 + content must equal total_budget."""
        for node_count, edge_count in [(10, 15), (200, 600), (1000, 4000)]:
            params = self._get_params(node_count, edge_count, total_budget=60_000)
            assert params.tier1_tokens + params.content_tokens == 60_000

    def test_anchor_count_bounds(self):
        """Anchor count should be clamped between 3 and 10."""
        for nc in [1, 5, 10, 50, 200, 1000]:
            params = self._get_params(nc, nc * 3)
            assert 3 <= params.anchor_count <= 10

    def test_neighborhood_cap_bounds(self):
        """Neighborhood cap should be clamped between 10 and 40."""
        for nc in [1, 10, 50, 200, 1000]:
            params = self._get_params(nc, nc * 3)
            assert 10 <= params.neighborhood_max_files <= 40

    def test_map_max_files_bounds(self):
        """map_max_files should be clamped between 20 and 75."""
        for nc in [1, 10, 50, 200, 1000]:
            params = self._get_params(nc, nc * 3)
            assert 20 <= params.map_max_files <= 75
