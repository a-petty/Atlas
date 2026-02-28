import collections
from dataclasses import dataclass
from pathlib import Path
from typing import List, Dict, Optional, Tuple
import tiktoken
import logging

from atlas.semantic_engine import RepoGraph, create_skeleton_from_source
from .embeddings import EmbeddingManager # Assuming EmbeddingManager is in embeddings.py

logger = logging.getLogger(__name__)

# Re-ranking weights for combining embedding similarity with PageRank.
SIMILARITY_WEIGHT = 0.80
PAGERANK_WEIGHT = 0.20

# Known context window sizes (tokens). Used to set max_tokens automatically.
# Conservative utilization: use 60% of window (leave room for system prompt + response).
MODEL_CONTEXT_WINDOWS = {
    # Cloud models
    "claude": 200_000,
    "claude-opus": 200_000,
    "claude-sonnet": 200_000,
    "gpt-4": 128_000,
    "gpt-4o": 128_000,
    "gemini": 1_000_000,
    "gemini-pro": 1_000_000,
    # Local models (Ollama)
    "deepseek-coder": 128_000,
    "deepseek-r2-distill-qwen-32b": 128_000,
    "codellama": 16_000,
    "llama3": 8_000,
    "mistral": 32_000,
    "qwen2.5-coder": 128_000,
    # Local models (MLX — HuggingFace model IDs)
    "mlx-community/deepseek-coder": 128_000,
    "mlx-community/mistral": 32_000,
    "mlx-community/codellama": 16_000,
    "mlx-community/llama-3": 8_000,
    "mlx-community/qwen2.5-coder": 128_000,
}

CONTEXT_UTILIZATION = 0.60  # Use 60% of window for context (rest for system prompt + response)
DEFAULT_CONTEXT_WINDOW = 100_000  # Fallback for unknown models


@dataclass
class ContextParams:
    """Adaptive context assembly parameters computed from repo characteristics."""
    tier1_tokens: int            # Map budget (measured actual, capped)
    content_tokens: int          # Skeleton content budget (all non-map)
    anchor_count: int            # Vector search top_n (3–10)
    map_max_files: int           # Ranked list length (20–75)
    neighborhood_max_hops: int   # BFS depth (2–3)
    neighborhood_max_files: int  # BFS results cap (10–40)

class ContextManager:
    """
    Manages context window for LLM queries using the
    "Anchor & Expand" strategy from the roadmap.
    """
    
    def __init__(
        self,
        repo_graph: RepoGraph,
        embedding_manager: EmbeddingManager,
        model: str = "gpt-4",
        max_tokens: Optional[int] = None
    ):
        self.repo_graph = repo_graph
        self.embedding_manager = embedding_manager
        try:
            self.encoder = tiktoken.encoding_for_model(model)
        except KeyError:
            # Model not known to tiktoken — fall back to cl100k_base (GPT-4 tokenizer)
            self.encoder = tiktoken.get_encoding("cl100k_base")
        self.max_tokens = self._resolve_max_tokens(model, max_tokens)
        logger.info(f"ContextManager initialized with model '{model}', max_tokens={self.max_tokens}")

    @staticmethod
    def _resolve_max_tokens(model: str, explicit_max_tokens: Optional[int]) -> int:
        """Determine max_tokens from model name, unless explicitly overridden."""
        if explicit_max_tokens is not None:
            return explicit_max_tokens

        # Try exact match, then prefix match (e.g., "deepseek-coder:7b" matches "deepseek-coder")
        model_lower = model.lower()
        window = MODEL_CONTEXT_WINDOWS.get(model_lower)
        if window is None:
            for prefix, w in MODEL_CONTEXT_WINDOWS.items():
                if model_lower.startswith(prefix):
                    window = w
                    break
        if window is None:
            window = DEFAULT_CONTEXT_WINDOW

        return int(window * CONTEXT_UTILIZATION)

    def _compute_adaptive_params(self, total_budget: int, map_text: str) -> ContextParams:
        """Compute adaptive context parameters based on repo size and map measurement."""
        stats = self.repo_graph.get_statistics()
        node_count = stats.node_count
        edge_count = stats.edge_count
        density = edge_count / max(node_count, 1)

        # Tier 1 (map): use actual measured tokens, capped at 8% of total budget
        map_tokens = self.count_tokens(map_text)
        tier1_tokens = min(map_tokens, int(total_budget * 0.08))

        # All remaining budget goes to skeleton content
        content_tokens = total_budget - tier1_tokens

        # Anchor count: scale with repo size, clamped 3–10
        anchor_count = min(max(3, node_count // 10), 10)

        # Map ranked list: scale with repo size, clamped 20–75
        map_max_files = min(max(20, node_count // 4), 75)

        # Neighborhood hops: sparse graphs get deeper traversal
        neighborhood_max_hops = 3 if density < 3.0 else 2

        # Neighborhood file cap: scale with repo size, clamped 10–40
        neighborhood_max_files = min(max(10, node_count // 5), 40)

        params = ContextParams(
            tier1_tokens=tier1_tokens,
            content_tokens=content_tokens,
            anchor_count=anchor_count,
            map_max_files=map_max_files,
            neighborhood_max_hops=neighborhood_max_hops,
            neighborhood_max_files=neighborhood_max_files,
        )

        logger.info(
            f"Adaptive params: repo={node_count} files, density={density:.1f}, "
            f"map={params.tier1_tokens}, content={params.content_tokens}, "
            f"anchors={params.anchor_count}, hops={params.neighborhood_max_hops}, "
            f"neighborhood_cap={params.neighborhood_max_files}"
        )

        return params
    
    def assemble_context(
        self,
        user_query: str,
        files_in_scope: List[Path],
        include_map: bool = True
    ) -> str:
        """
        Build context using skeleton-based budgeting.

        After a compact repository map, all remaining budget is filled with
        file skeletons (signatures + docstrings) — anchors first, then their
        dependency neighborhood, then extended context. The LLM can use Read
        to get full source for any file it needs to inspect further.
        """
        context_parts = []
        total_budget = self.max_tokens

        # Reserve for system prompt and query
        total_budget -= self.count_tokens(user_query)
        total_budget -= 1000  # System prompt overhead

        processed_files = set()

        # Step 1: Generate map (need it to measure tokens for adaptive params)
        stats = self.repo_graph.get_statistics()
        map_max_files = min(max(20, stats.node_count // 4), 75)
        map_text = self.repo_graph.generate_map(max_files=map_max_files) if include_map else ""

        # Step 2: Compute adaptive parameters
        params = self._compute_adaptive_params(total_budget, map_text)

        # === TIER 1: Repository Map (measured budget) ===
        if include_map and map_text:
            # Supplement with query-relevant file ranking when embeddings available
            if self.embedding_manager is not None and user_query:
                try:
                    stats = self.repo_graph.get_statistics()
                    all_graph_files = [Path(p) for p, _ in self.repo_graph.get_top_ranked_files(stats.node_count)]
                    relevant = self.embedding_manager.find_relevant_files(
                        user_query, all_graph_files, top_n=15
                    )
                    if relevant:
                        relevant_section = "\n\nQUERY-RELEVANT FILES (by semantic similarity):\n"
                        for i, p in enumerate(relevant, 1):
                            try:
                                rel_path = p.relative_to(self.repo_graph.project_root)
                            except (ValueError, AttributeError):
                                rel_path = p
                            relevant_section += f"  {i}. {rel_path}\n"
                        map_text = map_text + relevant_section
                except Exception as e:
                    logger.debug(f"Query-relevant map supplement failed: {e}")

            map_tokens = self.count_tokens(map_text)
            if map_tokens <= params.tier1_tokens:
                context_parts.append(("REPOSITORY_MAP", map_text))
                logger.debug(f"Added repo map ({map_tokens} tokens)")
            else:
                truncated_map = self._truncate_to_tokens(map_text, params.tier1_tokens)
                context_parts.append(("REPOSITORY_MAP (truncated)", truncated_map))
                logger.debug(f"Added truncated repo map ({params.tier1_tokens} tokens)")

        # === CONTENT: All skeletons, single budget ===
        content_budget = params.content_tokens

        # 1. Explicit files in scope (highest priority)
        explicit_content, explicit_processed = self._fill_with_content(
            files_in_scope,
            content_budget,
            processed_files,
            is_skeleton=True
        )
        if explicit_content:
            context_parts.append(("EXPLICIT_FILES (skeletons)", explicit_content))
            processed_files.update(explicit_processed)
            content_budget -= self.count_tokens(explicit_content)

        # 2. Vector search anchors (with similarity + PageRank re-ranking)
        stats = self.repo_graph.get_statistics()
        all_graph_files = [Path(p) for p, _ in self.repo_graph.get_top_ranked_files(stats.node_count)]
        scored = self.embedding_manager.find_relevant_files_scored(
            user_query,
            all_graph_files,
            top_n=params.anchor_count * 3,
        )
        pagerank_map = dict(self.repo_graph.get_top_ranked_files(stats.node_count))
        max_pr = max(pagerank_map.values()) if pagerank_map else 1.0
        reranked = []
        for path, similarity in scored:
            pr = pagerank_map.get(str(path), 0.0)
            normalized_pr = pr / max_pr if max_pr > 0 else 0.0
            combined = SIMILARITY_WEIGHT * similarity + PAGERANK_WEIGHT * normalized_pr
            reranked.append((path, combined))
        reranked.sort(key=lambda x: x[1], reverse=True)
        anchor_files = [path for path, _ in reranked[:params.anchor_count]]

        anchor_content, anchor_processed = self._fill_with_content(
            anchor_files,
            content_budget,
            processed_files,
            is_skeleton=True
        )
        if anchor_content:
            context_parts.append(("ANCHOR_FILES (skeletons)", anchor_content))
            processed_files.update(anchor_processed)
            content_budget -= self.count_tokens(anchor_content)

        # 3. Neighborhood expansion (dependencies + dependents of anchors)
        expansion_base = list(files_in_scope) + anchor_files
        weighted_neighborhood = self._get_dependency_neighborhood(
            expansion_base,
            processed_files,
            max_hops=params.neighborhood_max_hops,
            max_files=params.neighborhood_max_files,
        )
        neighborhood_paths = [path for path, _weight in weighted_neighborhood]

        neighborhood_content, neighborhood_processed = self._fill_with_content(
            neighborhood_paths,
            content_budget,
            processed_files,
            is_skeleton=True
        )
        if neighborhood_content:
            context_parts.append(("NEIGHBORHOOD_FILES (skeletons)", neighborhood_content))
            processed_files.update(neighborhood_processed)
            content_budget -= self.count_tokens(neighborhood_content)

        # 4. Extended neighbors (1-hop from everything above)
        all_seed_files = list(files_in_scope) + anchor_files + neighborhood_paths
        extended_neighbors = self._get_dependency_neighborhood(
            all_seed_files,
            processed_files,
            max_hops=1,
            max_files=100,
        )
        extended_candidates = [path for path, _weight in extended_neighbors]

        # If neighbor set is sparse, supplement with embedding similarity
        if len(extended_candidates) < 20:
            seen = set(extended_candidates)
            if self.embedding_manager is not None:
                try:
                    embedding_candidates = self.embedding_manager.find_relevant_files(
                        user_query, all_graph_files, top_n=50
                    )
                    for p in embedding_candidates:
                        if p not in seen and p not in processed_files:
                            extended_candidates.append(p)
                            seen.add(p)
                except Exception as e:
                    logger.debug(f"Embedding skeleton fallback failed: {e}")

            # Final fallback to PageRank if still sparse
            if len(extended_candidates) < 20:
                top_ranked = self.repo_graph.get_top_ranked_files(100)
                top_ranked_paths = [Path(p) for p, _ in top_ranked]
                for p in top_ranked_paths:
                    if p not in seen and p not in processed_files:
                        extended_candidates.append(p)
                        seen.add(p)

        # Filter noise files
        extended_candidates = [p for p in extended_candidates if not self._is_noise_file(p)]

        extended_content, extended_processed = self._fill_with_content(
            extended_candidates,
            content_budget,
            processed_files,
            is_skeleton=True
        )
        if extended_content:
            context_parts.append(("EXTENDED_CONTEXT (skeletons)", extended_content))
            processed_files.update(extended_processed)

        final_budget = total_budget - sum(self.count_tokens(c[1]) for c in context_parts)
        logger.info(f"Context assembled: {len(processed_files)} files, {final_budget} tokens remaining")

        return self._format_context(context_parts)

    @staticmethod
    def _is_noise_file(path: Path) -> bool:
        """Filter out files that provide no useful architectural context."""
        name = path.name
        # Empty __init__.py files
        if name == "__init__.py":
            try:
                content = path.read_text().strip()
                meaningful = "\n".join(
                    line for line in content.splitlines()
                    if line.strip() and not line.strip().startswith("#")
                )
                if len(meaningful) < 50:
                    return True
            except Exception:
                return True
        # Test files
        if name.startswith("test_") or name.endswith("_test.py"):
            return True
        if name == "conftest.py":
            return True
        return False

    def _get_dependency_neighborhood(
        self,
        anchor_files: List[Path],
        already_processed: set,
        max_hops: int = 2,
        max_files: int = 30
    ) -> List[Tuple[Path, float]]:
        """
        Build a neighborhood using multi-hop BFS with edge-type-aware traversal.

        Traversal rules:
        - SymbolUsage edges: follow for up to max_hops (real code dependencies)
        - Import edges: follow for 1 hop only (prevents transitive re-export explosion)

        Returns files sorted by weight descending, capped at max_files.
        Weight uses distance decay: hop 1 = 1.0, hop 2 = 0.5, hop 3 = 0.25, etc.
        """
        # file_path -> best (lowest hop) distance seen
        best_hop: Dict[Path, int] = {}
        # BFS queue: (file_path, current_hop, edge_kind_that_got_us_here)
        queue = collections.deque()

        for anchor in anchor_files:
            canonical = anchor.resolve()
            if canonical not in already_processed:
                queue.append((canonical, 0, None))

        visited_at_hop: Dict[Path, int] = {}
        for anchor in anchor_files:
            visited_at_hop[anchor.resolve()] = 0

        while queue:
            current_path, current_hop, arriving_edge = queue.popleft()

            if current_hop >= max_hops:
                continue

            next_hop = current_hop + 1
            canonical_str = str(current_path)

            # Expand both directions: dependencies and dependents
            neighbors = []
            try:
                neighbors.extend(self.repo_graph.get_dependencies(canonical_str))
            except Exception:
                pass
            try:
                neighbors.extend(self.repo_graph.get_dependents(canonical_str))
            except Exception:
                pass

            for neighbor_str, edge_kind in neighbors:
                neighbor_path = Path(neighbor_str)

                # Import edges: only follow at hop 0 -> 1 (1 hop max)
                if edge_kind == "Import" and current_hop >= 1:
                    continue

                # Skip if we already found this file at an equal or better hop
                if neighbor_path in visited_at_hop and visited_at_hop[neighbor_path] <= next_hop:
                    continue

                visited_at_hop[neighbor_path] = next_hop

                # Record best hop for weight calculation
                if neighbor_path not in best_hop or next_hop < best_hop[neighbor_path]:
                    best_hop[neighbor_path] = next_hop

                queue.append((neighbor_path, next_hop, edge_kind))

        # Remove anchors and already-processed files
        anchor_set = {a.resolve() for a in anchor_files}
        for remove_path in (already_processed | anchor_set):
            best_hop.pop(remove_path, None)

        # Compute weights with distance decay: hop n -> 1 / 2^(n-1)
        weighted: List[Tuple[Path, float]] = []
        for file_path, hop in best_hop.items():
            weight = 1.0 / (2 ** (hop - 1))
            weighted.append((file_path, weight))

        # Sort by weight descending, cap at max_files
        weighted.sort(key=lambda x: x[1], reverse=True)
        return weighted[:max_files]

    def _fill_with_content(
        self,
        file_list: List[Path],
        max_tokens: int,
        already_processed: set[Path],
        is_skeleton: bool = True
    ) -> Tuple[str, set[Path]]:
        """
        Fill token budget with either full content or skeletons.
        
        Args:
            file_list: Files to include
            max_tokens: Token budget
            already_processed: Files already in context
            is_skeleton: If True, use skeletons; if False, use full content
        """
        content_parts = []
        token_count = 0
        processed_in_this_call = set()
        
        for file_path in file_list:
            if token_count >= max_tokens:
                break
            if file_path in already_processed:
                continue
                
            try:
                source = file_path.read_text()
                
                if is_skeleton:
                    content = self._extract_signatures(source, file_path.suffix[1:])
                    label = f"# {file_path} (SKELETON)"
                else:
                    content = source
                    label = f"# {file_path} (FULL CONTENT)"
                
                if not content.strip():
                    continue
                    
                formatted = f"\n{label}\n{content}\n"
                tokens = self.count_tokens(formatted)
                
                if token_count + tokens <= max_tokens:
                    content_parts.append(formatted)
                    token_count += tokens
                    processed_in_this_call.add(file_path)
                else:
                    # Skip files that don't fit and try the next one
                    continue
                    
            except Exception as e:
                logger.debug(f"Could not process {file_path}: {e}")
                continue
        
        return "".join(content_parts), processed_in_this_call
    
    def _extract_signatures(self, content: str, lang_ext: str) -> str:
        """
        Extract function/class signatures from source code using the Rust core.
        """
        try:
            return create_skeleton_from_source(content, lang_ext)
        except Exception as e:
            logger.error(f"Error creating skeleton: {e}")
            # Fallback to a very simple signature extraction
            lines = content.split("\n")
            signatures = []
            for line in lines:
                stripped = line.strip()
                if stripped.startswith("def ") or stripped.startswith("class ") or stripped.startswith("async def "):
                    signatures.append(line)
                elif stripped.startswith("fn "): # Rust
                    signatures.append(line)
            return "\n".join(signatures)
    
    def _truncate_to_tokens(self, text: str, max_tokens: int) -> str:
        """Truncate text to fit within token budget."""
        tokens = self.encoder.encode(text)
        if len(tokens) <= max_tokens:
            return text
        
        truncated_tokens = tokens[:max_tokens]
        return self.encoder.decode(truncated_tokens) + "\n\n[... truncated ...]"
    
    def _format_context(self, parts: List[Tuple[str, str]]) -> str:
        """Format context parts into a single string."""
        formatted = []
        for label, content in parts:
            formatted.append(f"{'='*80}\n{label}\n{'='*80}\n{content}\n")
        return "\n".join(formatted)
    
    def count_tokens(self, text: str) -> int:
        """Count tokens in text."""
        return len(self.encoder.encode(text))