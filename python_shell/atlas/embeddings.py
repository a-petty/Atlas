# FastEmbed wrapper with skeleton-based chunk-level embeddings

from dataclasses import dataclass, field
from pathlib import Path
from typing import List, Dict, Tuple, Optional
import numpy as np
from fastembed import TextEmbedding
import logging
import hashlib
import os
import json
import importlib.metadata
import tempfile
import zipfile
import time

logger = logging.getLogger(__name__)

# Definition keywords that mark chunk boundaries when splitting skeleton text
_DEFINITION_PREFIXES = (
    "def ", "async def ", "class ",       # Python
    "fn ", "pub fn ", "impl ",            # Rust
    "function ", "export function ",      # JS/TS
    "export default function ",           # JS/TS
    "export class ", "export interface ",  # TS
    "func ",                              # Go
)

# Rough token-to-word ratio for estimating whether text fits in model context.
# BGE-small has a 512 token limit. Using ~400 as target to leave headroom.
_MAX_CHUNK_WORDS = 300  # ~400 tokens


@dataclass
class FileEmbedding:
    """Embeddings for a single file, split into semantic chunks."""
    chunk_embeddings: List[np.ndarray] = field(default_factory=list)
    content_key: str = ""


class EmbeddingManager:
    """
    Manages the generation and search of vector embeddings for code files.
    Uses the "Anchor" part of the Anchor & Expand strategy.

    When a repo_graph is provided, files are embedded using their skeleton
    representation (signatures + docstrings + imports, no function bodies)
    split into function-level chunks. This improves search precision by:
    - Fitting within the model's 512-token context window
    - Allowing individual functions to match queries independently
    """

    # Common source directory names that should be stripped from module paths
    _SOURCE_DIR_NAMES = {"src", "lib", "source", "sources"}

    def __init__(self, model_name: str = "BAAI/bge-small-en-v1.5", repo_graph=None, project_root: Optional[Path] = None, cache_dir: Optional[Path] = None, progress=None, cache_max_bytes: Optional[int] = None):
        logger.info(f"Initializing EmbeddingManager with model: {model_name}")
        try:
            self.model = TextEmbedding(model_name=model_name)
            self.embeddings_cache: Dict[Path, FileEmbedding] = {}
            self.repo_graph = repo_graph
            self.project_root = Path(project_root) if project_root is not None else None
            self.model_name = model_name
            self.progress = progress or (lambda **state: None)
            self.cache_dir = Path(cache_dir or os.environ.get("ATLAS_CACHE_DIR", Path.home() / ".cache" / "atlas")) / "embeddings-v2"
            self.engine_key = json.dumps([model_name, importlib.metadata.version("fastembed"),
                self._model_content_digest(), "atlas-skeleton-v2", "chunk-v3-bounded"])
            configured_budget = cache_max_bytes if cache_max_bytes is not None else os.environ.get(
                "ATLAS_EMBEDDING_CACHE_BYTES", 2 * 1024 ** 3)
            self.cache_max_bytes = int(configured_budget)
            if self.cache_max_bytes < 0:
                raise ValueError("embedding cache budget must be non-negative")
            self._disk_cache_bytes = 0
            self._cache_writes_since_check = 0
            self._prune_disk_cache()
            self._matrix_key = None
            self._matrix = None
            self._matrix_owners = None
            self._file_keys = {}
            self._expected_dimension = None
            # Use the model tokenizer to bound chunks before ONNX truncation.
            self._tokenizer = getattr(getattr(self.model, "model", None), "tokenizer", None)
            if type(self._tokenizer).__module__.startswith("tokenizers"):
                from tokenizers import Tokenizer
                self._tokenizer = Tokenizer.from_str(self._tokenizer.to_str())
                self._tokenizer.no_truncation()
            else:
                self._tokenizer = None
            logger.info("FastEmbed model initialized and ready.")
        except Exception as e:
            logger.error(f"Failed to initialize fastembed model {model_name}: {e}")
            raise

    def _model_content_digest(self):
        """Include actual local weights/config/tokenizer bytes in persistent keys.

        Streaming hashes avoid a second in-memory copy of model weights. Custom
        engines without local files use their class identity and FastEmbed/model
        version; those engines are responsible for stable model naming.
        """
        model_dir = getattr(getattr(self.model, "model", None), "_model_dir", None)
        if not isinstance(model_dir, (str, Path)):
            return f"custom:{type(self.model).__module__}.{type(self.model).__qualname__}"
        directory = Path(model_dir)
        digest = hashlib.sha256()
        for path in sorted(directory.rglob("*")):
            if not path.is_file() or path.name.endswith(".lock"):
                continue
            digest.update(str(path.relative_to(directory)).encode() + b"\0")
            with path.open("rb") as stream:
                for block in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(block)
        return digest.hexdigest()

    def _prune_disk_cache(self):
        """Oldest-accessed eviction. Cache reads touch mtime explicitly for LRU.

        Scans occur on startup, every 64 writes, and whenever this worker crosses
        the byte budget. Concurrent workers may evict each other's cache entries;
        cache misses simply rebuild immutable vectors.
        """
        # Hard-killed workers cannot execute their temporary-file finally block.
        # Remove only Atlas-owned temporaries from dead writers (or >24h old).
        for temporary in self.cache_dir.glob("embedding-*.tmp"):
            try:
                pid = int(temporary.name.split("-")[1])
                expired = time.time() - temporary.stat().st_mtime > 86400
                try:
                    os.kill(pid, 0)
                    dead = False
                except ProcessLookupError:
                    dead = True
                except PermissionError:
                    dead = False
                if dead or expired:
                    temporary.unlink(missing_ok=True)
            except (ValueError, FileNotFoundError):
                pass
        entries, total = [], 0
        for path in self.cache_dir.glob("*.npz"):
            try:
                stat = path.stat()
            except FileNotFoundError:
                continue
            total += stat.st_size
            entries.append((stat.st_mtime_ns, path, stat.st_size))
        for _, path, size in sorted(entries):
            if total <= self.cache_max_bytes:
                break
            try:
                path.unlink()
            except FileNotFoundError:
                pass
            total -= size
        self._disk_cache_bytes = max(0, total)
        self._cache_writes_since_check = 0

    def _file_path_to_module_prefix(self, file_path: Path) -> str:
        """Convert a file path to a dotted module prefix for embedding.

        Examples:
            /repo/airflow-core/src/airflow/models/pool.py → airflow.models.pool
            /repo/src/utils/helpers.py → utils.helpers
            /repo/mypackage/__init__.py → mypackage
        """
        if self.project_root is None:
            return ""
        try:
            rel = file_path.resolve().relative_to(self.project_root.resolve())
        except ValueError:
            return ""

        parts = list(rel.parts)
        if not parts:
            return ""

        # Strip common source directory names wherever they appear (e.g., src/, lib/)
        # This handles both top-level (src/airflow/...) and nested
        # monorepo layouts (airflow-core/src/airflow/...)
        parts = [p for p in parts if p.lower() not in self._SOURCE_DIR_NAMES]

        if not parts:
            return ""

        # Remove file extension from last part
        last = parts[-1]
        stem = Path(last).stem
        if stem == "__init__":
            parts = parts[:-1]
        else:
            parts[-1] = stem

        return ".".join(parts)

    def _get_embedding_text(self, file_path: Path) -> str:
        """Get text to embed for a file.

        Uses skeleton if repo_graph is available, and prepends the dotted
        module path (e.g., 'airflow.models.pool') so that file identity
        is captured in the embedding.
        """
        text = None
        if self.repo_graph is not None:
            # A real snapshot reader is authoritative. Generic MagicMock attributes
            # and old graph implementations retain the historical disk fallback.
            accepted_reader = getattr(self.repo_graph, "get_source", None)
            authoritative = callable(getattr(type(self.repo_graph), "get_source", None))
            try:
                skeleton = self.repo_graph.get_skeleton(str(file_path))
                if isinstance(skeleton, str) and skeleton.strip():
                    text = skeleton
            except Exception:
                if authoritative:
                    raise
            if text is None and authoritative:
                text = accepted_reader(str(file_path))
                if not isinstance(text, str):
                    raise ValueError("file is not in the accepted source generation")
        if text is None:
            text = file_path.read_text()

        # Prepend module path prefix for semantic signal
        prefix = self._file_path_to_module_prefix(file_path)
        if prefix:
            text = f"{prefix}\n{text}"

        return text

    def _split_into_chunks(self, text: str) -> List[str]:
        """Split text into chunks at function/class definition boundaries.

        If the text is short enough to fit in a single model context window,
        returns it as a single chunk. Otherwise, splits on definition keywords
        so each function/class gets its own embedding.
        """
        if len(text.split()) <= _MAX_CHUNK_WORDS:
            return [text]

        lines = text.split("\n")
        chunks: List[str] = []
        current_lines: List[str] = []

        for line in lines:
            stripped = line.strip()
            is_definition = any(stripped.startswith(kw) for kw in _DEFINITION_PREFIXES)

            if is_definition and current_lines:
                chunk_text = "\n".join(current_lines).strip()
                if chunk_text:
                    chunks.append(chunk_text)
                current_lines = []

            current_lines.append(line)

        # Flush remaining lines
        if current_lines:
            chunk_text = "\n".join(current_lines).strip()
            if chunk_text:
                chunks.append(chunk_text)

        return chunks if chunks else [text]

    def _file_similarity(self, query_embedding: np.ndarray, file_entry: FileEmbedding) -> float:
        """Compute similarity between a query and a file using max-chunk scoring.

        Returns the highest cosine similarity across all chunks, so a file
        ranks highly if *any* of its functions match the query well.
        """
        if not file_entry.chunk_embeddings:
            return 0.0
        return max(
            self._cosine_similarity(query_embedding, chunk_emb)
            for chunk_emb in file_entry.chunk_embeddings
        )

    def generate_embedding(self, texts: List[str]) -> List[np.ndarray]:
        """Generates embeddings for a list of text inputs.

        Sorts inputs by length before batching to minimize ONNX padding overhead.
        ONNX pads all inputs in a batch to the longest sequence; without sorting,
        a single long text forces every item in that batch to process at max length.
        """
        if not texts:
            return []
        try:
            # Sort by length so similarly-sized texts batch together
            indexed = sorted(enumerate(texts), key=lambda x: len(x[1]))
            sorted_texts = [t for _, t in indexed]

            sorted_embeddings = []
            for offset in range(0, len(sorted_texts), 64):
                batch = sorted_texts[offset:offset + 64]
                vectors = list(self.model.embed(batch))
                if len(vectors) != len(batch):
                    raise ValueError("Embedding engine returned a partial batch")
                for vector in vectors:
                    vector = np.asarray(vector, dtype=np.float32)
                    if vector.ndim != 1 or not vector.size or not np.isfinite(vector).all():
                        raise ValueError("Embedding engine returned an invalid vector")
                    if self._expected_dimension is None:
                        self._expected_dimension = vector.size
                    elif vector.size != self._expected_dimension:
                        raise ValueError("Embedding engine changed vector dimensions")
                    sorted_embeddings.append(vector)

            # Restore original order
            embeddings = [None] * len(texts)
            for i, (orig_idx, _) in enumerate(indexed):
                embeddings[orig_idx] = sorted_embeddings[i]

            return embeddings
        except Exception as e:
            logger.error(f"Failed to generate embeddings for texts: {e}")
            raise

    def _cosine_similarity(self, vec1: np.ndarray, vec2: np.ndarray) -> float:
        """Calculates cosine similarity between two vectors."""
        norm1 = np.linalg.norm(vec1)
        norm2 = np.linalg.norm(vec2)
        if norm1 == 0 or norm2 == 0:
            return 0.0
        return float(np.dot(vec1, vec2) / (norm1 * norm2))

    def _bounded_chunks(self, text):
        """Enforce the embedding model's input length even for one huge declaration."""
        output = []
        for chunk in self._split_into_chunks(text):
            if self._tokenizer is None:
                # Offline test doubles and compatible engines without a tokenizer.
                words = chunk.split()
                output.extend([" ".join(words[i:i+300]) for i in range(0, len(words), 300)] or [chunk])
                continue
            offsets = self._tokenizer.encode(chunk, add_special_tokens=False).offsets
            if len(offsets) <= 480:
                output.append(chunk)
                continue
            for i in range(0, len(offsets), 480):
                part = offsets[i:i+480]
                output.append(chunk[part[0][0]:part[-1][1]])
        return output

    def _load_cached(self, key):
        try:
            with np.load(self.cache_dir / (key + ".npz"), allow_pickle=False) as cached:
                vectors = cached["vectors"]
            if (vectors.ndim != 2 or not all(vectors.shape) or not np.isfinite(vectors).all()
                    or (self._expected_dimension is not None and vectors.shape[1] != self._expected_dimension)):
                return None
            try:
                os.utime(self.cache_dir / (key + ".npz"), None)
            except FileNotFoundError:
                pass
            return FileEmbedding(list(vectors), key)
        except (OSError, ValueError, KeyError, zipfile.BadZipFile):
            return None

    def _save_cached(self, entry):
        if self.cache_max_bytes == 0:
            return
        self.cache_dir.mkdir(parents=True, exist_ok=True)
        destination = self.cache_dir / (entry.content_key + ".npz")
        fd, temporary = tempfile.mkstemp(dir=self.cache_dir, prefix=f"embedding-{os.getpid()}-", suffix=".tmp")
        try:
            with os.fdopen(fd, "wb") as stream:
                np.savez_compressed(stream, vectors=np.asarray(entry.chunk_embeddings, dtype=np.float32))
                stream.flush()
                os.fsync(stream.fileno())
            size = Path(temporary).stat().st_size
            # Oversized entries remain useful in memory without evicting the
            # entire disk cache or exceeding its configured budget.
            if size > self.cache_max_bytes:
                return
            try:
                previous_size = destination.stat().st_size
            except FileNotFoundError:
                previous_size = 0
            os.replace(temporary, destination)
            self._disk_cache_bytes += size - previous_size
            self._cache_writes_since_check += 1
            if self._disk_cache_bytes > self.cache_max_bytes or self._cache_writes_since_check >= 64:
                self._prune_disk_cache()
        finally:
            Path(temporary).unlink(missing_ok=True)

    def invalidate(self, paths=None):
        if paths is None:
            self._file_keys.clear()
        else:
            for path in paths:
                self._file_keys.pop(Path(path), None)
        self._matrix_key = None

    def prepare(self, file_paths):
        paths = list(dict.fromkeys(Path(p) for p in file_paths))
        live = set(paths)
        for path in list(self.embeddings_cache):
            if path not in live:
                self.embeddings_cache.pop(path, None)
                self._file_keys.pop(path, None)
        complete = 0
        batch, size = [], 0
        self.progress(phase="preparing_embeddings", completed=0, total=len(paths))

        def flush():
            nonlocal complete, batch, size
            if not batch:
                return
            vectors = self.generate_embedding([chunk for _, _, chunks in batch for chunk in chunks])
            offset = 0
            for path, key, chunks in batch:
                entry = FileEmbedding(vectors[offset:offset + len(chunks)], key)
                offset += len(chunks)
                self._save_cached(entry)
                self.embeddings_cache[path] = entry
                complete += 1
            batch, size = [], 0
            self.progress(phase="preparing_embeddings", completed=complete, total=len(paths))

        for path in paths:
            # The session invalidates accepted edits. Standalone callers hash the
            # actual representation on every use. Keep at most one pending batch
            # of source text, rather than retaining the entire repository twice.
            key = self._file_keys.get(path) if getattr(self, "generation_managed", False) else None
            text = None
            if key is None:
                text = self._get_embedding_text(path)
                key = hashlib.sha256((self.engine_key + "\0" + text).encode()).hexdigest()
                self._file_keys[path] = key
            entry = self.embeddings_cache.get(path)
            if entry is None or entry.content_key != key:
                entry = self._load_cached(key)
            if entry is not None and entry.content_key == key:
                self.embeddings_cache[path] = entry
                complete += 1
                continue
            self.embeddings_cache.pop(path, None)
            chunks = self._bounded_chunks(text if text is not None else self._get_embedding_text(path))
            if size + len(chunks) > 64:
                flush()
            batch.append((path, key, chunks)); size += len(chunks)
            if size >= 64:
                flush()
        flush()
        matrix_key = tuple((str(p), self._file_keys[p]) for p in paths)
        if matrix_key != self._matrix_key:
            vectors = [vector for path in paths for vector in self.embeddings_cache[path].chunk_embeddings]
            counts = [len(self.embeddings_cache[path].chunk_embeddings) for path in paths]
            self._matrix = np.stack(vectors).astype(np.float32, copy=False) if vectors else np.empty((0, 0), dtype=np.float32)
            if len(self._matrix):
                # einsum computes row norms without allocating a matrix-sized
                # square temporary. np.stack performs the one necessary copy.
                norms = np.sqrt(np.einsum("ij,ij->i", self._matrix, self._matrix))
                self._matrix /= np.maximum(norms[:, None], 1e-12)
            self._matrix_owners = np.repeat(np.arange(len(paths), dtype=np.int64), counts)
            self._matrix_key = matrix_key
        self.progress(phase="ready", completed=complete, total=len(paths))
        return paths

    def find_relevant_files_scored(self, query: str, file_paths: List[Path], top_n: int = 5) -> List[Tuple[Path, float]]:
        if top_n < 0:
            raise ValueError("top_n must be non-negative")
        if not file_paths or top_n == 0:
            return []
        # Query first preserves the public call ordering used by custom engines.
        query_chunks = self._bounded_chunks(query)
        query_vectors = np.asarray(self.generate_embedding(query_chunks), dtype=np.float32)
        # Long queries are represented in full via bounded chunks, rather than
        # silently allowing the model to truncate the tail at 512 tokens.
        vector = np.mean(query_vectors, axis=0)
        paths = self.prepare(file_paths)
        vector /= max(float(np.linalg.norm(vector)), 1e-12)
        scores = np.full(len(paths), -np.inf, dtype=np.float32)
        np.maximum.at(scores, self._matrix_owners, self._matrix @ vector)
        ranked = [(path, float(score)) for path, score in zip(paths, scores)]
        ranked.sort(key=lambda item: (-item[1], str(item[0])))
        return ranked[:top_n]

    def find_relevant_files(self, query: str, file_paths: List[Path], top_n: int = 5) -> List[Path]:
        """
        Finds files most relevant to a given query using cosine similarity.

        Convenience wrapper around find_relevant_files_scored() that returns
        just the file paths without scores.
        """
        return [path for path, _ in self.find_relevant_files_scored(query, file_paths, top_n)]
