import collections
import json
import hashlib
from dataclasses import dataclass, field
from pathlib import Path
from typing import List, Dict, Optional, Tuple
import tiktoken
import logging

from atlas.semantic_engine import RepoGraph, create_skeleton_from_source
from atlas import semantic_engine
from .embeddings import EmbeddingManager # Assuming EmbeddingManager is in embeddings.py

logger = logging.getLogger(__name__)

# Re-ranking weights for combining embedding similarity with PageRank.
SIMILARITY_WEIGHT = 0.80
PAGERANK_WEIGHT = 0.20
_METADATA_CACHE_MAX_ENTRIES = 16_384
_METADATA_CACHE_MAX_KEY_CHARS = 2048

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
    tier1_tokens: int
    content_tokens: int
    anchor_count: int
    map_max_files: int
    neighborhood_max_hops: int
    neighborhood_max_files: int
    full_source_tokens: int = 0
    skeleton_tokens: int = 0


@dataclass
class ContextChunk:
    file: str
    mode: str
    text: str
    source_spans: list[dict]
    token_count: int
    selection_reason: str


@dataclass
class ContextResult:
    """The manifest describes only source actually delivered in ``text``.

    Character offsets locate chunk content in that text. Source offsets are
    half-open UTF-8 byte ranges; source lines are 1-based and inclusive.
    ``json.dumps(result.to_dict())`` is also covered by the transport ceiling.
    """
    generation: object
    text: str
    chunks: list[dict]
    omissions: list[dict]
    counts: dict

    def to_dict(self) -> dict:
        return {"generation": self.generation, "text": self.text,
                "chunks": self.chunks, "omissions": self.omissions,
                "counts": self.counts}


class ContextManager:
    """Rank once, then pack map, full sources, and complete skeleton chunks."""

    def __init__(self, repo_graph: RepoGraph, embedding_manager: EmbeddingManager,
                 model: str = "gpt-4", max_tokens: Optional[int] = None):
        self.repo_graph = repo_graph
        self.embedding_manager = embedding_manager
        try:
            self.encoder = tiktoken.encoding_for_model(model)
        except KeyError:
            self.encoder = tiktoken.get_encoding("cl100k_base")
        self.max_tokens = self._resolve_max_tokens(model, max_tokens)
        self._skeleton_cache = {}
        self._minimum_chunk_cache = collections.OrderedDict()

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
        stats = self.repo_graph.get_statistics()
        total_budget = max(0, total_budget)
        map_tokens = min(self.count_tokens(map_text), int(total_budget * .08))
        content = total_budget - map_tokens
        full = int(content * max(.40, min(.75, .75 - stats.node_count / 1500)))
        return ContextParams(map_tokens, content,
                             min(max(3, stats.node_count // 10), 10),
                             min(max(20, stats.node_count // 4), 75),
                             3 if stats.edge_count / max(stats.node_count, 1) < 3 else 2,
                             min(max(10, stats.node_count // 5), 40), full, content - full)

    def _source(self, path: Path) -> str:
        reader = getattr(self.repo_graph, "get_source", None)
        if callable(reader):
            source = reader(str(path))
            if source is None:
                raise ValueError("file is not in the accepted source generation")
            return source
        # Compatibility with old extensions and isolated test doubles only.
        return path.read_text()

    def _canonical(self, path) -> Path:
        # Paths returned by the graph are accepted-generation identities. Resolve
        # only external aliases; touching the live filesystem again for accepted
        # paths is both expensive and wrong if a symlink changes mid-response.
        indexed = getattr(self, "_indexed_paths", {})
        key = str(path)
        if key in indexed:
            return indexed[key]
        path = Path(path)
        root = getattr(self.repo_graph, "project_root", None)
        if not path.is_absolute() and root is not None:
            path = Path(root) / path
        return indexed.get(str(path)) or path.resolve()

    def _index_paths(self, ranked=None):
        if ranked is None:
            ranked = self.repo_graph.get_top_ranked_files(self.repo_graph.get_statistics().node_count)
        self._indexed_paths = {str(path): Path(path) for path, _ in ranked}
        return ranked

    def _all_ranked(self):
        ranked = self._index_paths()
        return sorted(ranked, key=lambda item: (-item[1], str(item[0])))

    def ranked_candidates(self, query, files_in_scope, expand=True):
        """Return ordered (path, reason) candidates; no retrieval-only file cap."""
        ranked = self._all_ranked()
        all_files = [self._canonical(p) for p, _ in ranked]
        params = self._compute_adaptive_params(self.max_tokens, "")
        scored = []
        if self.embedding_manager is not None and query:
            scored = self.embedding_manager.find_relevant_files_scored(
                query, all_files, top_n=len(all_files))
        pagerank = {self._canonical(p): score for p, score in ranked}
        max_pr = max(pagerank.values(), default=1.) or 1.
        scored.sort(key=lambda pair: (-pair[1], str(pair[0])))
        top = scored[:params.anchor_count * 3]
        top = sorted(top, key=lambda pair: (
            -(SIMILARITY_WEIGHT * pair[1] + PAGERANK_WEIGHT *
              pagerank.get(self._canonical(pair[0]), 0.) / max_pr), str(pair[0])))
        anchors = [self._canonical(p) for p, _ in top[:params.anchor_count]]
        explicit = [self._canonical(p) for p in files_in_scope]
        neighbors = []
        if expand:
            neighbors = [p for p, _ in self._get_dependency_neighborhood(
                explicit + anchors, set(), params.neighborhood_max_hops, None)]
        candidates = [(p, "explicit") for p in explicit]
        candidates += [(p, "anchor") for p in anchors]
        candidates += [(p, "neighborhood") for p in neighbors]
        candidates += [(self._canonical(p), "extended") for p, _ in scored]
        candidates += [(p, "architecture") for p in all_files]
        return candidates

    def assemble_context(self, user_query, files_in_scope, include_map=True) -> str:
        return self.assemble_context_result(user_query, files_in_scope, include_map).text

    def assemble_context_result(self, user_query, files_in_scope, include_map=True,
                                max_tokens=None, max_chars=60_000, generation=None,
                                expand=True) -> ContextResult:
        return self.pack_ranked_context_result(
            user_query, self.ranked_candidates(user_query, files_in_scope, expand),
            include_map=include_map, max_tokens=max_tokens,
            max_chars=max_chars, generation=generation)

    def _skeleton_chunks(self, source, path):
        key = (path.suffix, hashlib.sha256(source.encode()).hexdigest())
        if key in self._skeleton_cache:
            return self._skeleton_cache[key]
        extractor = getattr(semantic_engine, "create_skeleton_details_json", None)
        if extractor is None:
            # Old binary fallback must not manufacture retained-span credit.
            text = self._extract_signatures(source, path.suffix[1:])
            chunks = [{"text": text, "spans": []}] if text.strip() else []
        else:
            chunks = json.loads(extractor(source, path.suffix[1:]))["chunks"]
        if len(self._skeleton_cache) >= 512:
            self._skeleton_cache.pop(next(iter(self._skeleton_cache)))
        self._skeleton_cache[key] = chunks
        return chunks

    def _render_result(self, chunks, map_text, omissions, generation, omission_detail_limit=0):
        parts = []
        if generation is not None:
            parts.append(f"SOURCE GENERATION: {generation}\n")
        if map_text:
            parts.append("REPOSITORY_MAP\n" + map_text + "\n")
        manifest = []
        offset = sum(map(len, parts))
        for chunk in chunks:
            mode = "FULL CONTENT" if chunk.mode == "full" else "SKELETON"
            header = f"\n# {chunk.file} ({mode})\n"
            start = offset + len(header)
            parts.append(header + chunk.text + "\n")
            offset += len(parts[-1])
            manifest.append({"file": chunk.file, "mode": chunk.mode,
                "source_spans": chunk.source_spans,
                "token_count": chunk.token_count,
                "selection_reason": chunk.selection_reason,
                "start_char": start, "end_char": start + len(chunk.text)})
        omission_counts = dict(collections.Counter(o["reason"] for o in omissions))
        if omissions:
            parts.append("\nOMISSIONS: " + ", ".join(
                f"{reason}={count}" for reason, count in sorted(omission_counts.items())) + "\n")
        text = "".join(parts)
        # Bounded detail; counts expose every omitted candidate/chunk.
        details = omissions[:omission_detail_limit]
        return ContextResult(generation, text, manifest, details, {
            "files": len({c.file for c in chunks}), "chunks": len(chunks),
            "tokens": self.count_tokens(text), "characters": len(text),
            "omitted": len(omissions), "omission_reasons": omission_counts,
            "omission_details_omitted": max(0, len(omissions) - len(details))})

    def _minimum_chunk_overhead(self, path, reason):
        """Source-independent metadata lower bound, with bounded warm reuse."""
        path = str(path)
        structured = getattr(semantic_engine, "create_skeleton_details_json", None) is not None
        key = (path, reason, structured)
        cached = self._minimum_chunk_cache.get(key)
        if cached is not None:
            self._minimum_chunk_cache.move_to_end(key)
            return cached
        # Every current nonempty structured chunk retains original bytes. The
        # old string-only fallback does not expose that source-span evidence.
        descriptor = {"file": path, "mode": "", "source_spans": [], "token_count": 0,
            "selection_reason": reason, "start_char": 99999999999999999999,
            "end_char": 99999999999999999999}
        costs = []
        for mode in ("full", "skeleton"):
            descriptor["mode"] = mode
            descriptor["source_spans"] = ([{"start_byte": 0, "end_byte": 1,
                "start_line": 1, "end_line": 1}] if mode == "full" or structured else [])
            encoded_descriptor = json.dumps(descriptor)
            label = "FULL CONTENT" if mode == "full" else "SKELETON"
            empty_addition = json.dumps(f"\n# {path} ({label})\n\n")
            costs.append((self.count_tokens(encoded_descriptor) + self.count_tokens(empty_addition),
                          len(encoded_descriptor) + len(empty_addition) - 2))
        value = (min(cost[0] for cost in costs), min(cost[1] for cost in costs))
        # Limit entry count and key size. Paths/reasons beyond this cap still
        # receive identical accounting, without occupying the warm cache.
        if len(path) + len(reason) <= _METADATA_CACHE_MAX_KEY_CHARS:
            self._minimum_chunk_cache[key] = value
            if len(self._minimum_chunk_cache) > _METADATA_CACHE_MAX_ENTRIES:
                self._minimum_chunk_cache.popitem(last=False)
        return value

    def pack_ranked_context_result(self, user_query, candidates, include_map=True,
                                   max_tokens=None, max_chars=60_000, generation=None):
        """Identical rendering/representation policy for Atlas and baselines.

        The budget includes the query and 1,000 tokens of prompt allowance.
        Both text and default JSON serialization fit the token and character ceilings.
        If the query/prompt allowance exhausts the budget, return status with no source.
        Entire declarations are admitted or omitted, never sliced.
        """
        if max_chars < 512:
            raise ValueError("max_chars must be at least 512 for result metadata")
        limit = self.max_tokens if max_tokens is None else max_tokens
        if limit < 0:
            raise ValueError("max_tokens must be non-negative")
        available = max(0, limit - self.count_tokens(user_query) - 1000)
        self._index_paths()
        unique, seen = [], set()
        for path, reason in candidates:
            path = self._canonical(path)
            if path not in seen:
                unique.append((path, reason)); seen.add(path)
        map_text = ""
        if include_map:
            map_limit = int(available * .08)
            stats = self.repo_graph.get_statistics()
            raw = self.repo_graph.generate_map(max_files=min(max(20, stats.node_count // 4), 75))
            for line in raw.splitlines(keepends=True):
                proposed = map_text + line
                if self.count_tokens("REPOSITORY_MAP\n" + proposed + "\n") > map_limit:
                    break
                map_text = proposed
        params = self._compute_adaptive_params(available, "REPOSITORY_MAP\n" + map_text if map_text else "")
        chunks, omissions = [], []
        full_used = 0

        # Rejected candidates must not re-tokenize/re-serialize the entire context.
        # Cache the accepted prefix; account conservatively for the added whole
        # chunk. The final exact renderer checks both budgets again.
        fit_cache = {}
        def refresh_prefix():
            prefix_key = (len(chunks), min(len(omissions), 8))
            if fit_cache.get("key") != prefix_key:
                baseline = self._render_result(chunks, map_text, omissions, generation)
                fit_cache.update(key=prefix_key, tokens=baseline.counts["tokens"],
                                 json_chars=len(json.dumps(baseline.to_dict())),
                                 json_tokens=self.count_tokens(json.dumps(baseline.to_dict())))

        def fits(proposed):
            refresh_prefix()
            chunk = proposed[-1]
            mode = "FULL CONTENT" if chunk.mode == "full" else "SKELETON"
            addition = f"\n# {chunk.file} ({mode})\n{chunk.text}\n"
            # 20-digit offsets upper-bound actual offsets under the transport cap.
            descriptor = {"file": chunk.file, "mode": chunk.mode,
                "source_spans": chunk.source_spans, "token_count": chunk.token_count,
                "selection_reason": chunk.selection_reason,
                "start_char": 99999999999999999999, "end_char": 99999999999999999999}
            extra_chars = len(json.dumps(addition)) - 2 + len(json.dumps(descriptor)) + 2
            extra_tokens = self.count_tokens(json.dumps(addition)) + self.count_tokens(json.dumps(descriptor)) + 2
            return (fit_cache["tokens"] + self.count_tokens(addition) <= max(0, available - 128)
                    and fit_cache["json_tokens"] + extra_tokens <= max(0, available - 128)
                    and fit_cache["json_chars"] + extra_chars <= max_chars - 768)

        for candidate_index, (path, reason) in enumerate(unique):
            refresh_prefix()
            # Every source chunk needs a manifest descriptor and retained span,
            # irrespective of how short its declaration is. If these cannot fit,
            # parsing this candidate's entire file cannot improve the result.
            # Include the mandatory file header and cheapest valid source span.
            minimum_tokens, minimum_chars = self._minimum_chunk_overhead(path, reason)
            if (fit_cache["json_tokens"] + minimum_tokens + 2 > max(0, available - 128)
                    or fit_cache["json_chars"] + minimum_chars + 2 > max_chars - 768):
                omissions.append({"file": str(path), "reason": "budget", "mode": "file"})
                continue
            try:
                source = self._source(path)
            except Exception as exc:
                omissions.append({"file": str(path), "reason": "source_unavailable"})
                logger.debug("Cannot read accepted source %s: %s", path, exc)
                continue
            if not source.strip():
                continue
            raw = source.encode()
            spans = [{"start_byte": 0, "end_byte": len(raw), "start_line": 1,
                      "end_line": source.count("\n") + (0 if source.endswith("\n") else 1)}]
            chunk = ContextChunk(str(path), "full", source, spans,
                                 self.count_tokens(source), reason)
            cost = self.count_tokens(f"\n# {path} (FULL CONTENT)\n{source}\n")
            if full_used + cost <= params.full_source_tokens and fits(chunks + [chunk]):
                chunks.append(chunk); full_used += cost
                continue
            # Admit a high-priority fallback before lower-priority full files.
            # Otherwise architecture files can consume the entire transport
            # budget while an explicit file that fits as a skeleton is omitted.
            try:
                pieces = self._skeleton_chunks(source, path)
            except Exception as exc:
                omissions.append({"file": str(path), "reason": "skeleton_unavailable"})
                logger.debug("Cannot skeletonize %s: %s", path, exc)
                continue
            if not pieces:
                omissions.append({"file": str(path), "reason": "empty_skeleton"})
            for piece in pieces:
                text = piece["text"]
                if not text.strip():
                    continue
                chunk = ContextChunk(str(path), "skeleton", text, piece["spans"],
                                     self.count_tokens(text), reason)
                if fits(chunks + [chunk]):
                    chunks.append(chunk)
                else:
                    omissions.append({"file": str(path), "reason": "budget",
                                      "mode": "skeleton"})
        result = self._render_result(chunks, map_text, omissions, generation)
        # Only complete chunks/map lines may be removed to fit final metadata.
        while (result.counts["tokens"] > available or
               ((chunks or map_text) and self.count_tokens(json.dumps(result.to_dict())) > available) or
               len(json.dumps(result.to_dict())) > max_chars or len(result.text) > max_chars):
            if chunks:
                removed = chunks.pop()
                omissions.append({"file": removed.file, "reason": "transport_or_token_budget"})
            elif map_text:
                map_text = "".join(map_text.splitlines(keepends=True)[:-1])
            else:
                # Extremely small token budgets cannot carry even an omissions footer.
                result.text = ""
                result.counts.update(tokens=0, characters=0)
                result.omissions = []
                result.counts["omission_details_omitted"] = len(omissions)
                if len(json.dumps(result.to_dict())) > max_chars:
                    raise ValueError("max_chars too small for generation/result metadata")
                break
            result = self._render_result(chunks, map_text, omissions, generation)
        # Diagnostic paths consume only the space left after source admission.
        # Omission counts are always retained, even when no details fit. This
        # prevents lower-priority diagnostics displacing an explicit API chunk.
        if chunks or map_text:
            for detail_limit in range(1, min(8, len(omissions)) + 1):
                detailed = self._render_result(chunks, map_text, omissions, generation,
                                               omission_detail_limit=detail_limit)
                encoded = json.dumps(detailed.to_dict())
                if (detailed.counts["tokens"] > available or self.count_tokens(encoded) > available
                        or len(encoded) > max_chars or len(detailed.text) > max_chars):
                    break
                result = detailed
        return result

    def _get_dependency_neighborhood(self, anchor_files, already_processed,
                                     max_hops=2, max_files=30):
        anchors = {self._canonical(p) for p in anchor_files}
        excluded = {self._canonical(p) for p in already_processed} | anchors
        # Content inclusion and graph visitation are deliberately independent.
        queue = collections.deque((path, 0) for path in sorted(anchors))
        visited = {path: 0 for path in anchors}
        best_hop = {}
        while queue:
            path, hop = queue.popleft()
            if hop >= max_hops:
                continue
            neighbors = collections.defaultdict(set)
            for reader in (self.repo_graph.get_dependencies, self.repo_graph.get_dependents):
                for neighbor, kind in reader(str(path)):
                    neighbors[self._canonical(neighbor)].add(kind)
            for neighbor, kinds in sorted(neighbors.items()):
                if hop >= 1 and "SymbolUsage" not in kinds:
                    continue
                if visited.get(neighbor, max_hops + 1) <= hop + 1:
                    continue
                visited[neighbor] = hop + 1
                best_hop[neighbor] = hop + 1
                queue.append((neighbor, hop + 1))
        weighted = [(p, 1. / 2 ** (h - 1)) for p, h in best_hop.items() if p not in excluded]
        weighted.sort(key=lambda item: (-item[1], str(item[0])))
        return weighted if max_files is None else weighted[:max_files]

    def _fill_with_content(self, file_list, max_tokens, already_processed, is_skeleton=True):
        """Legacy helper; coherent source reads and complete files only."""
        parts, processed = [], set()
        for path in file_list:
            path = self._canonical(path)
            if path in already_processed:
                continue
            try:
                source = self._source(path)
                content = self._extract_signatures(source, path.suffix[1:]) if is_skeleton else source
                mode = "SKELETON" if is_skeleton else "FULL CONTENT"
                text = f"\n# {path} ({mode})\n{content}\n"
                if content.strip() and self.count_tokens("".join(parts) + text) <= max_tokens:
                    parts.append(text); processed.add(path)
            except Exception as exc:
                logger.debug("Cannot process %s: %s", path, exc)
        return "".join(parts), processed

    def _extract_signatures(self, content, lang_ext):
        return create_skeleton_from_source(content, lang_ext)

    def count_tokens(self, text):
        try:
            return len(self.encoder.encode(text, disallowed_special=()))
        except TypeError:
            return len(self.encoder.encode(text))
