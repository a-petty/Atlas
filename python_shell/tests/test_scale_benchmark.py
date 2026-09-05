"""Fixture/protocol tests, without substituting a small fixture for scale results."""
import hashlib
import importlib.util
from pathlib import Path

import pytest

_SCRIPT = Path(__file__).resolve().parents[2] / 'benchmarks' / 'bench_scale.py'
spec = importlib.util.spec_from_file_location('atlas_scale_fixture', _SCRIPT)
scale = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scale)


def test_mixed_fixture_is_exact_deterministic_and_content_hashed(tmp_path):
    root = tmp_path / 'fixture'
    one = scale.create_fixture(root, file_count=25, seed=123)
    timestamps = {item['path']: (root / item['path']).stat().st_mtime_ns for item in one['files']}
    two = scale.create_fixture(root, file_count=25, seed=123)
    assert one == two
    assert len(one['files']) == one['file_count'] == 25
    assert set(one['files_by_language'].values()) == {5}
    for item in one['files']:
        content = (root / item['path']).read_bytes()
        assert hashlib.sha256(content).hexdigest() == item['sha256']
        assert len(content) == item['bytes']
        assert (root / item['path']).stat().st_mtime_ns == timestamps[item['path']]
    changed = scale.create_fixture(tmp_path / 'different_seed', file_count=25, seed=124)
    assert changed['fixture_sha256'] != one['fixture_sha256']


def test_fixture_refuses_unrelated_nonempty_directory(tmp_path):
    (tmp_path / 'important.txt').write_text('keep me')
    with pytest.raises(ValueError, match='Refusing nonempty'):
        scale.create_fixture(tmp_path, file_count=10)
    assert (tmp_path / 'important.txt').read_text() == 'keep me'


def test_p95_nearest_rank_and_measured_target_are_explicit():
    result = scale.summarize(list(range(1, 21)), target=18)
    assert result['p95_seconds'] == 19 and not result['meets_target']
    assert result['median_seconds'] == 10.5 and result['sample_count'] == 20
    assert scale.percentile95([]) is None


def test_real_engine_accepts_all_five_fixture_languages(tmp_path):
    from atlas.session import RepositoryState
    scale.create_fixture(tmp_path / 'fixture', file_count=25)
    state = RepositoryState(tmp_path / 'fixture')
    try:
        result = state.execute('status', {})
        assert result['coverage']['indexed'] == 25
        assert result['coverage']['invalid_count'] == 0
        assert result['statistics']['file_counts_by_language'] == {
            'python': 5, 'javascript': 5, 'typescript': 5, 'rust': 5, 'go': 5}
        dependencies = state.execute('dependencies', {'file_path': 'python/module_0000.py'})
        assert any(item['file'].endswith('/python/module_0001.py') for item in dependencies['items'])
    finally:
        state.close()


def test_public_edit_preserves_docstring_and_restorable_original(monkeypatch):
    import ast
    monkeypatch.syspath_prepend(str(_SCRIPT.parent))
    path = _SCRIPT.parent / 'bench_public_scale.py'
    public_spec = importlib.util.spec_from_file_location('atlas_public_scale', path)
    module = importlib.util.module_from_spec(public_spec)
    public_spec.loader.exec_module(module)
    original = b'def api(value):\r\n    """API doc."""\r\n    return value\r\n'
    changed = module.insertion_edit(original, 9)
    tree = ast.parse(changed.decode())
    assert ast.get_docstring(tree.body[0]) == 'API doc.'
    assert b'_atlas_scale_iteration = 9\r\n' in changed
    assert original == b'def api(value):\r\n    """API doc."""\r\n    return value\r\n'
