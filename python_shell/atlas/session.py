"""A serialized, restartable repository session shared by MCP and the CLI.

The child process owns every mutable graph/index. A request first reconciles the
filesystem manifest, accepts captured buffers as one generation, then answers
only from those buffers. Killing and joining that process is the cancellation
boundary; an abandoned thread cannot continue mutating the next generation.
"""
from __future__ import annotations

import atexit
import base64
import fnmatch
import hashlib
import json
import logging
import multiprocessing as mp
import os
import resource
import sys
from pathlib import Path
import threading
import time
from types import SimpleNamespace
import uuid

log = logging.getLogger(__name__)
IGNORED_DIRS = {".git", ".venv", "venv", "node_modules", "target", "__pycache__", "dist", "build"}
EXTENSIONS = {".py", ".pyi", ".rs", ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".go"}
CONFIG_NAMES = {".atlas.toml", ".atlasignore", ".gitignore", "pyproject.toml", "package.json", "pyrightconfig.json", "jsconfig.json"}
MAX_RESULT_CHARS = 60_000


def _signature(path):
    stat = path.stat()
    return (stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns, stat.st_ino)


def _config_name(path):
    return path.name in CONFIG_NAMES or fnmatch.fnmatch(path.name, "tsconfig*.json") or fnmatch.fnmatch(path.name, "requirements*.txt")


def _ignore_match(relative, patterns):
    ignored = False
    for raw in patterns:
        negate = raw.startswith("!")
        pattern = raw[1:] if negate else raw
        pattern = pattern.strip("/")
        if not pattern:
            continue
        match = (fnmatch.fnmatch(relative, pattern) or fnmatch.fnmatch(relative, pattern + "/**")
                 or ("/" not in pattern and any(fnmatch.fnmatch(part, pattern) for part in relative.split("/"))))
        if match:
            ignored = not negate
    return ignored


class RepositoryState:
    """Worker-owned state. Direct construction is useful for deterministic tests."""
    def __init__(self, root, progress=None):
        self.root = Path(root).resolve(strict=True)
        self.progress = progress or (lambda **state: None)
        self.epoch = uuid.uuid4().hex[:12]
        self.revision = 0
        self.generation = None
        self.manifest = {}
        self.config_manifest = {}
        self.sources = {}
        self.invalid = {}
        self.graph = None
        self.embeddings = None
        self.context = None
        self.watcher = None
        self.watcher_error = None
        try:
            from atlas.semantic_engine import FileWatcher
            self.watcher = FileWatcher(str(self.root), extensions=[e[1:] for e in sorted(EXTENSIONS)], ignored_dirs=sorted(IGNORED_DIRS))
        except Exception as exc:
            self.watcher_error = str(exc)
        self._cpg_files = []

    def close(self):
        if self.watcher is not None:
            self.watcher.stop()
            self.watcher = None

    def _scan(self):
        from atlas.semantic_engine import scan_repository
        patterns = []
        ignore = self.root / ".atlasignore"
        if ignore.exists():
            patterns = [line.strip() for line in ignore.read_text().splitlines() if line.strip() and not line.lstrip().startswith("#")]
        files, configs = {}, {}
        paths = {Path(p) for p in scan_repository(str(self.root), ignored_dirs=sorted(IGNORED_DIRS), include_aliases=True)}
        paths.update(self.root / name for name in CONFIG_NAMES)
        for path in paths:
            # Ignore external symlink targets and generated trees even when tracked.
            if not path.is_relative_to(self.root):
                continue
            rel = path.relative_to(self.root)
            if any(part in IGNORED_DIRS for part in rel.parts):
                continue
            if path.is_symlink():
                alias_stat = path.lstat()
                configs["alias:" + str(path)] = (alias_stat.st_ino, alias_stat.st_ctime_ns, os.readlink(path), str(path.resolve()))
                # Actual source files arrive separately under canonical IDs.
                continue
            try:
                signature = _signature(path)
            except FileNotFoundError:
                continue
            if _config_name(path):
                configs[str(path)] = signature
            if path.suffix.lower() in EXTENSIONS and not _ignore_match(rel.as_posix(), patterns):
                files[str(path)] = signature
        return files, configs

    def _new_graph(self, sources):
        from atlas.semantic_engine import RepoGraph
        config = self.root / ".atlas.toml"
        kwargs = {"ignored_dirs": sorted(IGNORED_DIRS)}
        if config.exists():
            try:
                import tomllib
            except ImportError:
                import tomli as tomllib
            project = tomllib.loads(config.read_text()).get("project", {})
            for key in ("source_roots", "languages"):
                value = project.get(key)
                if value is not None:
                    if not isinstance(value, list) or any(not isinstance(item, str) for item in value):
                        raise ValueError(f".atlas.toml project.{key} must be an array of strings")
                    kwargs[key] = value
        graph = RepoGraph(str(self.root), **kwargs)
        graph.build_from_sources(sources)
        return graph

    def reconcile(self, force=False):
        # Hints improve responsiveness; correctness never depends on delivery.
        if self.watcher is not None:
            self.watcher.poll_events()
        for attempt in range(3):
            manifest, configs = self._scan()
            if not force and manifest == self.manifest and configs == self.config_manifest and self.graph is not None:
                return False
            changed = {p for p, signature in manifest.items() if self.manifest.get(p) != signature}
            removed = set(self.manifest) - set(manifest)
            rebuild = force or self.graph is None or configs != self.config_manifest or bool(removed) or bool(set(manifest) - set(self.manifest))
            self.progress(phase="capturing_sources", completed=0, total=len(changed), generation=self.generation)
            sources = {p: text for p, text in self.sources.items() if p in manifest}
            invalid = {p: reason for p, reason in self.invalid.items() if p in manifest and p not in changed}
            unstable = False
            for index, path in enumerate(sorted(changed)):
                try:
                    source = Path(path).read_bytes().decode("utf-8")
                    if _signature(Path(path)) != manifest[path]:
                        unstable = True; break
                    sources[path] = source
                except (OSError, UnicodeError) as exc:
                    sources.pop(path, None)
                    invalid[path] = f"unreadable: {exc}"
            if unstable:
                continue
            self.progress(phase="indexing", completed=0, total=len(sources), generation=self.generation)
            if rebuild:
                graph = self._new_graph(sources)
            else:
                graph = self.graph
                for path in sorted(changed):
                    if path not in sources:
                        try: graph.remove_file(path)
                        except Exception: pass
                        continue
                    try:
                        # A previously invalid file is absent from the graph.
                        if path in self.invalid:
                            graph.add_file(path, sources[path])
                        else:
                            graph.update_file(path, sources[path])
                    except Exception:
                        # Rebuild from the captured set establishes per-file parse
                        # coverage without retaining any stale declarations.
                        graph = self._new_graph(sources)
                        rebuild = True
                        break
            accepted = {p for p, _ in graph.get_top_ranked_files(graph.get_statistics().node_count)}
            for path in sources:
                if path not in accepted:
                    invalid[path] = "syntax_error: excluded from graph and retrieval"
                else:
                    invalid.pop(path, None)
            # Resolvers read project configuration/module paths at construction.
            # Verify that those inputs and captured file metadata stayed stable.
            after, after_configs = self._scan()
            if after != manifest or after_configs != configs:
                # An incrementally mutated graph must not be used on retry.
                self.graph = None
                force = True
                continue
            self.graph = graph
            self.sources = sources
            self.invalid = invalid
            self.manifest, self.config_manifest = manifest, configs
            self.revision += 1
            self.generation = f"{self.epoch}:{self.revision}"
            if self.embeddings is not None:
                self.embeddings.repo_graph = graph
                self.embeddings.invalidate(None if rebuild else changed | removed)
            if self.context is not None:
                self.context.repo_graph = graph
            if rebuild:
                self._cpg_files.clear()
            self.progress(phase="ready", completed=len(accepted), total=len(manifest), generation=self.generation)
            return True
        raise RuntimeError("Repository changed repeatedly during capture; retry when the current write completes")

    def coverage(self):
        result = {"indexed": self.graph.get_statistics().node_count, "eligible": len(self.manifest),
            "invalid_count": len(self.invalid), "invalid_files": {}, "invalid_details_omitted": len(self.invalid),
            "import_resolvers": list(self.graph.get_statistics().registered_resolvers),
            "call_analysis": "Python/JS/TS static lexical/import bindings; dynamic calls unresolved",
            "watcher": "native hints plus manifest reconciliation" if self.watcher else "manifest reconciliation"}
        if self.watcher_error:
            result["watcher_error"] = "Native watcher unavailable; manifest reconciliation remains active"
        # Admit complete diagnostic records. Very long paths cannot displace the
        # coverage counts or inflate every context/page beyond its transport cap.
        for path, reason in sorted(self.invalid.items())[:12]:
            result["invalid_files"][path] = reason.split(":", 1)[0]
            result["invalid_details_omitted"] -= 1
            if len(json.dumps(result)) > 1800:
                result["invalid_files"].pop(path)
                result["invalid_details_omitted"] += 1
        return result

    def _path(self, value):
        path = Path(os.path.abspath(self.root / value))
        aliases = [(Path(key[6:]), record[-1]) for key, record in self.config_manifest.items() if key.startswith("alias:")]
        for alias, target in sorted(aliases, key=lambda pair: len(pair[0].parts), reverse=True):
            if path == alias or alias in path.parents:
                path = Path(target) / path.relative_to(alias)
                break
        if not path.is_relative_to(self.root):
            raise ValueError("Path is outside this repository")
        return str(path)

    def _ensure_embeddings(self):
        if self.embeddings is None:
            from atlas.embeddings import EmbeddingManager
            self.progress(phase="loading_embedding_model", completed=0, total=len(self.manifest))
            self.embeddings = EmbeddingManager(repo_graph=self.graph, project_root=self.root, progress=self.progress)
            self.embeddings.generation_managed = True
        if self.context is None:
            from atlas.context import ContextManager
            self.context = ContextManager(self.graph, self.embeddings, max_tokens=12_000)

    def _page(self, items, operation, params, cursor=None, limit=100):
        if not isinstance(limit, int) or not 1 <= limit <= 1000:
            raise ValueError("limit must be between 1 and 1000")
        fingerprint = hashlib.sha256(json.dumps([operation, params], sort_keys=True).encode()).hexdigest()[:16]
        offset = 0
        if cursor:
            try:
                if not cursor.startswith("v1."): raise ValueError("Invalid cursor version")
                encoded = cursor[3:]
                value = json.loads(base64.urlsafe_b64decode(encoded + "=" * (-len(encoded) % 4)))
                if value[0] != self.generation:
                    raise ValueError("Stale cursor: repository generation changed; restart pagination")
                if value[1] != fingerprint:
                    raise ValueError("Cursor belongs to another query")
                offset = int(value[2])
                if offset < 0 or offset > len(items): raise ValueError("Invalid cursor offset")
            except (TypeError, IndexError, json.JSONDecodeError):
                raise ValueError("Invalid cursor") from None
        result = {"generation": self.generation, "coverage": self.coverage(), "items": [], "total": len(items), "next_cursor": None}
        for item in items[offset:offset+limit]:
            result["items"].append(item)
            next_offset = offset + len(result["items"])
            result["next_cursor"] = "v1." + base64.urlsafe_b64encode(json.dumps([self.generation, fingerprint, next_offset]).encode()).decode().rstrip("=") if next_offset < len(items) else None
            if len(json.dumps(result)) > MAX_RESULT_CHARS - 1024:
                result["items"].pop()
                if not result["items"]:
                    raise ValueError("One result exceeds transport capacity; request context with a smaller declaration")
                next_offset -= 1
                result["next_cursor"] = "v1." + base64.urlsafe_b64encode(json.dumps([self.generation, fingerprint, next_offset]).encode()).decode().rstrip("=")
                break
        return result

    def execute(self, operation, params):
        self.reconcile(force=operation == "refresh")
        if operation in {"status", "refresh"}:
            stats = self.graph.get_statistics()
            names = ["node_count", "edge_count", "import_edges", "symbol_edges", "total_definitions", "unresolved_import_count", "module_index_size", "source_roots", "known_root_modules", "registered_resolvers", "file_counts_by_language", "unsupported_language_counts"]
            statistics = {key: getattr(stats, key) for key in names}
            # Resolver selection does not suppress the observed language picture.
            language_names = {".py": "python", ".pyi": "python", ".rs": "rust", ".go": "go",
                ".js": "javascript", ".mjs": "javascript", ".cjs": "javascript", ".jsx": "javascript_jsx",
                ".ts": "typescript", ".tsx": "typescript_tsx"}
            counts = {}
            for path in self.manifest:
                language = language_names[Path(path).suffix.lower()]
                counts[language] = counts.get(language, 0) + 1
            resolvers = set(statistics["registered_resolvers"])
            unsupported = {language: count for language, count in counts.items()
                if (language == "python" and "python" not in resolvers)
                    or (language.startswith(("javascript", "typescript")) and "javascript_typescript" not in resolvers)
                    or language in {"rust", "go"}}
            statistics["file_counts_by_language"] = dict(sorted(counts.items()))
            statistics["unsupported_language_counts"] = dict(sorted(unsupported.items()))
            for key in ("source_roots", "known_root_modules"):
                values = statistics[key]
                statistics[key + "_total"] = len(values)
                kept = []
                for value in values:
                    if len(json.dumps(kept + [value])) > 4000: break
                    kept.append(value)
                statistics[key] = kept
                statistics[key + "_omitted"] = len(values) - len(kept)
            return {"generation": self.generation, "phase": "ready", "root": str(self.root), "statistics": statistics, "coverage": self.coverage(), "embeddings_loaded": self.embeddings is not None, "runtime": {"pid": os.getpid(), "peak_rss_bytes": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * (1 if sys.platform == "darwin" else 1024)}}
        if operation in {"dependencies", "dependents", "symbols", "skeleton", "callers", "callees"}:
            self.graph.get_source(self._path(params["file_path"]))
        if operation == "cpg_diagnostic":
            self.progress(phase="building_cpg", completed=0, total=len(self.manifest))
            self.graph.enable_cpg_and_build(excluded_dirs=sorted(IGNORED_DIRS))
            return {"generation": self.generation, "phase": "complete", "coverage": self.coverage()}
        if operation == "context":
            self._ensure_embeddings()
            files = [Path(self._path(p)) for p in params.get("files_in_scope", [])]
            coverage = self.coverage()
            overhead = len(json.dumps({"coverage": coverage})) + 16
            coverage_tokens = self.context.count_tokens(json.dumps({"coverage": coverage}) + "\nCOVERAGE: ") + 16
            result = self.context.assemble_context_result(params["query"], files,
                include_map=params.get("include_map", True), max_tokens=max(0, params.get("max_tokens", 12_000)-coverage_tokens),
                max_chars=MAX_RESULT_CHARS-overhead, generation=self.generation).to_dict()
            result["coverage"] = coverage
            return result
        if operation == "search":
            self._ensure_embeddings()
            paths = [Path(p) for p, _ in self.graph.get_top_ranked_files(self.graph.get_statistics().node_count)]
            ranked = self.embeddings.find_relevant_files_scored(params["query"], paths, len(paths))
            items = [{"file": str(p), "score": score} for p, score in ranked]
        elif operation in {"dependencies", "dependents"}:
            method = getattr(self.graph, "get_" + operation)
            items = [{"file": p, "kind": kind} for p, kind in sorted(method(self._path(params["file_path"])))]
        elif operation in {"callers", "callees"}:
            method = getattr(self.graph, "get_" + operation)
            items = method(self._path(params["file_path"]), params["function_name"])
            items = sorted(items, key=lambda item: (item["file"], item["line"], item["name"]))
        elif operation == "symbols":
            items = self.graph.get_file_symbols(self._path(params["file_path"]))
        elif operation == "skeleton":
            from atlas.semantic_engine import create_skeleton_details_json
            path = self._path(params["file_path"])
            items = json.loads(create_skeleton_details_json(self.graph.get_source(path), Path(path).suffix[1:]))["chunks"]
        elif operation == "ranked":
            items = [{"file": p, "rank": rank} for p, rank in self.graph.get_top_ranked_files(self.graph.get_statistics().node_count)]
        elif operation == "map":
            # Ranked complete lines allow generation-bound continuation rather than slicing a tree.
            items = [{"file": str(Path(p).relative_to(self.root)), "rank": rank} for p, rank in self.graph.get_top_ranked_files(self.graph.get_statistics().node_count)]
        elif operation == "graph":
            method = params["method"]
            allowed = {"get_source", "get_skeleton", "get_dependencies", "get_dependents", "get_top_ranked_files", "get_statistics", "generate_map", "get_callers", "get_callees", "get_callables"}
            if method not in allowed: raise ValueError("Unsupported graph operation")
            value = getattr(self.graph, method)(*params.get("args", []), **params.get("kwargs", {}))
            if method == "get_statistics":
                return self.execute("status", {})["statistics"]
            return value
        else:
            raise ValueError(f"Unknown operation: {operation}")
        query_params = {k: v for k, v in params.items() if k not in {"cursor", "limit"}}
        result = self._page(items, operation, query_params, params.get("cursor"), params.get("limit", 100))
        if operation in {"callers", "callees"}:
            candidates = [f for f in self.graph.get_callables(self._path(params["file_path"]))
                          if params["function_name"] in {f["name"], f["qualified_name"]}]
            result["query_resolution"] = {"matches": len(candidates), "ambiguous": len(candidates) > 1,
                "qualified_names": sorted({f["qualified_name"] for f in candidates})[:12],
                "semantics": "union of matching callables; use qualified_name to narrow"}
        return result


def _worker(connection, root):
    state = None
    try:
        state = RepositoryState(root, lambda **status: connection.send({"progress": status}))
        while True:
            message = connection.recv()
            if message is None: break
            try:
                value = state.execute(message["operation"], message["params"])
                connection.send({"result": value})
            except Exception as exc:
                log.exception("Repository request failed")
                connection.send({"error": f"{type(exc).__name__}: {exc}"})
    except (EOFError, BrokenPipeError):
        pass
    finally:
        if state is not None: state.close()
        connection.close()


class RepositorySession:
    def __init__(self, root, timeout=120, worker_target=None):
        self.root = Path(root).resolve()
        self.timeout = timeout
        self._worker_target = worker_target or _worker
        self._lock = threading.Lock()
        self._process_lock = threading.RLock()
        self._active_cancel_state = None
        self._process = None
        self._connection = None
        self.progress = {"phase": "not_started"}
        self.busy = False
        atexit.register(self.close)

    def _start(self):
        with self._process_lock:
            if self._process is not None and self._process.is_alive(): return
            self._terminate()
            context = mp.get_context("spawn")
            parent, child = context.Pipe()
            process = context.Process(target=self._worker_target, args=(child, str(self.root)), daemon=True)
            process.start()
            child.close()
            self._process, self._connection = process, parent

    def _terminate(self):
        with self._process_lock:
            process, connection = self._process, self._connection
            self._process = self._connection = None
            if process is not None:
                if process.is_alive(): process.terminate()
                process.join(timeout=3)
                if process.is_alive(): process.kill(); process.join()
                process.close()
            if connection is not None: connection.close()

    def _cancel(self, state):
        state.set()
        if self._active_cancel_state is state:
            self._terminate()

    def request(self, operation, _cancel_state=None, **params):
        state = _cancel_state or threading.Event()
        with self._lock:
            self._active_cancel_state = state
            self.busy = True
            try:
                if state.is_set():
                    raise RuntimeError("Request cancelled before it started")
                self._start()
                if state.is_set():
                    self._terminate()
                    raise RuntimeError("Request cancelled before it started")
                connection = self._connection
                connection.send({"operation": operation, "params": params})
                deadline = time.monotonic() + self.timeout
                while True:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        self._terminate()
                        self.progress = {"phase": "timed_out", "operation": operation}
                        raise TimeoutError(f"{operation} exceeded {self.timeout}s; worker terminated and joined. Retry starts a clean generation.")
                    if not connection.poll(min(.1, remaining)): continue
                    reply = connection.recv()
                    if "progress" in reply:
                        self.progress = reply["progress"]
                        log.info("Atlas preparation: %s", self.progress)
                        continue
                    if "error" in reply: raise RuntimeError(reply["error"])
                    return reply["result"]
            except TimeoutError:
                raise
            except (EOFError, BrokenPipeError, OSError):
                self._terminate()
                raise RuntimeError("Repository worker stopped; retry starts a clean generation") from None
            finally:
                self.busy = False
                self._active_cancel_state = None

    async def arequest(self, operation, **params):
        import anyio
        state = threading.Event()
        try:
            return await anyio.to_thread.run_sync(lambda: self.request(operation, _cancel_state=state, **params), abandon_on_cancel=True)
        except anyio.get_cancelled_exc_class():
            # A cancelled queued request must not terminate another caller's work.
            self._cancel(state)
            raise

    def close(self):
        with self._lock:
            if self._connection is not None:
                try: self._connection.send(None)
                except (OSError, BrokenPipeError): pass
            if self._process is not None: self._process.join(timeout=1)
            self._terminate()


class SessionGraph:
    """Read-only compatibility facade for existing CLI tools."""
    def __init__(self, session):
        self.session = session
        self.project_root = str(session.root)

    def __getattr__(self, method):
        def call(*args, **kwargs):
            if method in {"build_complete", "ensure_pagerank_up_to_date", "update_file", "add_file", "remove_file"}:
                return self.session.request("refresh")
            value = self.session.request("graph", method=method, args=list(args), kwargs=kwargs)
            return SimpleNamespace(**value) if method == "get_statistics" else value
        return call


class SessionContext:
    def __init__(self, session, max_tokens=12_000):
        self.session, self.max_tokens = session, max_tokens

    def assemble_context(self, query, files_in_scope, include_map=True):
        return self.session.request("context", query=query, files_in_scope=[str(p) for p in files_in_scope], include_map=include_map, max_tokens=self.max_tokens)["text"]
