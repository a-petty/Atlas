"""Persistence, generation, and model-length boundaries for embedding search."""
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import numpy as np
import pytest

from atlas.embeddings import EmbeddingManager


class DeterministicModel:
    def __init__(self):
        self.calls = []
        self.fail = False
    def embed(self, texts):
        self.calls.append(list(texts))
        if self.fail:
            raise RuntimeError('model unavailable')
        return [np.array([1 + text.count('alpha'), 1 + text.count('beta'), 1.], dtype=np.float32) for text in texts]


def manager(tmp_path, model=None, **kwargs):
    model = model or DeterministicModel()
    with patch('atlas.embeddings.TextEmbedding', return_value=model):
        return EmbeddingManager(cache_dir=tmp_path/'cache', **kwargs), model


def test_standalone_edit_invalidates_vector_and_restart_reuses_cache(tmp_path):
    path = tmp_path/'a.py'; path.write_text('alpha alpha')
    first, model = manager(tmp_path)
    first.find_relevant_files('alpha', [path])
    old = first.embeddings_cache[path].content_key
    path.write_text('beta beta')
    first.find_relevant_files('alpha', [path])
    assert first.embeddings_cache[path].content_key != old
    restarted, engine = manager(tmp_path)
    restarted.find_relevant_files('alpha', [path])
    assert len(engine.calls) == 1  # Query only; accepted file vector came from disk.
    assert restarted.embeddings_cache[path].content_key == first.embeddings_cache[path].content_key


def test_generation_invalidates_only_accepted_change_and_renames_prefix(tmp_path):
    first_path = tmp_path/'alpha.py'; first_path.write_text('same source')
    service, engine = manager(tmp_path, project_root=tmp_path)
    service.generation_managed = True
    service.find_relevant_files('alpha', [first_path])
    old = service.embeddings_cache[first_path].content_key
    second_path = tmp_path/'beta.py'; first_path.rename(second_path)
    service.invalidate([first_path, second_path])
    service.find_relevant_files('alpha', [second_path])
    assert first_path not in service.embeddings_cache and first_path not in service._file_keys
    assert service.embeddings_cache[second_path].content_key != old
    assert service._matrix_key[0][0] == str(second_path)


def test_no_disk_fallback_for_missing_accepted_source(tmp_path):
    path = tmp_path/'a.py'; path.write_text('live but unaccepted source')
    class Snapshot:
        def get_skeleton(self, path): return ''
        def get_source(self, path): return None
    service, _ = manager(tmp_path, repo_graph=Snapshot())
    with pytest.raises(ValueError, match='accepted'):
        service.find_relevant_files('alpha', [path])


def test_accepted_module_prefix_ignores_live_symlink_replacement(tmp_path):
    from atlas.semantic_engine import RepoGraph
    first, second = tmp_path / 'alpha.py', tmp_path / 'beta.py'
    first.write_text('def alpha():\n    return 1\n')
    second.write_text('def beta():\n    return 2\n')
    graph = RepoGraph(str(tmp_path))
    graph.build_from_sources({str(first): first.read_text(), str(second): second.read_text()})
    service, _ = manager(tmp_path, repo_graph=graph, project_root=tmp_path)
    captured = service._get_embedding_text(first)
    first.unlink(); first.symlink_to(second)
    assert service._file_path_to_module_prefix(first) == 'alpha'
    assert service._get_embedding_text(first) == captured
    assert captured.startswith('alpha\n') and 'def alpha' in captured
    # Standalone callers still deliberately read the live symlink target.
    standalone, _ = manager(tmp_path, project_root=tmp_path)
    assert standalone._file_path_to_module_prefix(first) == 'beta'


def test_failed_preparation_never_returns_partial_ranking(tmp_path):
    paths = []
    for i in range(70):
        path = tmp_path/f'f{i}.py'; path.write_text(f'alpha {i}'); paths.append(path)
    service, engine = manager(tmp_path)
    original = engine.embed
    def fail_later(texts):
        if len(engine.calls) >= 2:
            raise RuntimeError('interrupted second file batch')
        return original(texts)
    engine.embed = fail_later
    with pytest.raises(RuntimeError, match='interrupted'):
        service.find_relevant_files('alpha', paths)
    assert service._matrix is None
    assert len(service.embeddings_cache) == 64
    engine.embed = original
    assert len(service.find_relevant_files('alpha', paths, top_n=70)) == 70


def test_model_batch_limit_includes_huge_single_declaration(tmp_path):
    service, engine = manager(tmp_path)
    vectors = service.generate_embedding([str(i) for i in range(150)])
    assert len(vectors) == 150
    assert all(len(batch) <= 64 for batch in engine.calls)


def test_tokenizer_bounds_punctuation_unicode_and_full_query(tmp_path):
    service, engine = manager(tmp_path)
    class CharTokenizer:
        def encode(self, text, add_special_tokens=False):
            return SimpleNamespace(offsets=[(i, i+1) for i in range(len(text))])
    service._tokenizer = CharTokenizer()
    text = ('λ.,/' * 400) + 'tail beta'
    chunks = service._bounded_chunks(text)
    assert all(len(c) <= 480 for c in chunks)
    assert ''.join(chunks) == text
    path = tmp_path/'a.py'; path.write_text('alpha')
    service.find_relevant_files(text, [path])
    query_calls = engine.calls[0]
    assert any('tail beta' in c for c in query_calls)
    assert all(len(c) <= 480 for c in query_calls)


def test_corrupt_cache_is_rebuilt_and_atomic_write_failure_keeps_old_file(tmp_path, monkeypatch):
    path = tmp_path/'a.py'; path.write_text('alpha')
    service, _ = manager(tmp_path)
    service.find_relevant_files('alpha', [path])
    entry = service.embeddings_cache[path]
    cache_path = service.cache_dir/(entry.content_key+'.npz')
    cache_path.write_bytes(b'invalid archive')
    restarted, engine = manager(tmp_path)
    restarted.find_relevant_files('alpha', [path])
    assert len(engine.calls) == 2
    good = cache_path.read_bytes()
    def fail_replace(*args): raise OSError('replace interrupted')
    monkeypatch.setattr('atlas.embeddings.os.replace', fail_replace)
    with pytest.raises(OSError, match='interrupted'):
        restarted._save_cached(entry)
    assert cache_path.read_bytes() == good
    assert list(service.cache_dir.glob('*.npz')) == [cache_path]


def test_partial_or_nonfinite_engine_output_is_rejected(tmp_path):
    service, engine = manager(tmp_path)
    engine.embed = lambda texts: []
    with pytest.raises(ValueError, match='partial'):
        service.generate_embedding(['alpha'])
    engine.embed = lambda texts: [np.array([np.nan])]
    with pytest.raises(ValueError, match='invalid'):
        service.generate_embedding(['alpha'])


def test_disk_cache_has_budget_evicts_oldest_and_can_disable_persistence(tmp_path):
    service, _ = manager(tmp_path, cache_max_bytes=800)
    paths = []
    for i in range(10):
        path = tmp_path/f'{i}.py'; path.write_text(f'alpha {i}'); paths.append(path)
    service.prepare(paths)
    cached = list(service.cache_dir.glob('*.npz'))
    assert sum(path.stat().st_size for path in cached) <= 800
    assert len(cached) < len(paths)
    assert len(service.embeddings_cache) == len(paths)
    last_key = service.embeddings_cache[paths[-1]].content_key
    assert service.cache_dir.joinpath(last_key+'.npz').exists()
    disabled, _ = manager(tmp_path/'disabled', cache_max_bytes=0)
    disabled.prepare(paths)
    assert not list(disabled.cache_dir.glob('*.npz'))
    assert len(disabled.embeddings_cache) == len(paths)


def test_model_weight_or_config_change_changes_persistent_key(tmp_path):
    model_dir = tmp_path/'model'; model_dir.mkdir()
    (model_dir/'model.onnx').write_bytes(b'weights version1')
    (model_dir/'config.json').write_text('{}')
    engine = DeterministicModel(); engine.model = SimpleNamespace(_model_dir=model_dir)
    first, _ = manager(tmp_path, model=engine)
    (model_dir/'model.onnx').write_bytes(b'weights version2')
    second, _ = manager(tmp_path, model=engine)
    assert first.engine_key != second.engine_key
    (model_dir/'config.json').write_text('{"dimension": 3}')
    third, _ = manager(tmp_path, model=engine)
    assert second.engine_key != third.engine_key


def test_startup_removes_orphaned_temporary_from_dead_worker(tmp_path, monkeypatch):
    import atlas.embeddings as module
    directory = tmp_path/'cache'/'embeddings-v2'; directory.mkdir(parents=True)
    orphan = directory/'embedding-999999-orphan.tmp'; orphan.write_bytes(b'partial model output')
    def dead(pid, signal): raise ProcessLookupError()
    monkeypatch.setattr(module.os, 'kill', dead)
    manager(tmp_path)
    assert not orphan.exists()
