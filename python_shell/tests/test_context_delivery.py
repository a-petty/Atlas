"""Context correctness at the delivered-source and transport boundaries."""
import json
from pathlib import Path
from types import SimpleNamespace

from atlas.context import ContextManager
from atlas.semantic_engine import RepoGraph


class SnapshotGraph:
    def __init__(self, root, sources):
        self.project_root = str(root)
        self.sources = {str(root / path): source for path, source in sources.items()}
        self.edges = {}
    def get_source(self, path):
        return self.sources.get(path)
    def get_statistics(self):
        return SimpleNamespace(node_count=len(self.sources), edge_count=3)
    def get_top_ranked_files(self, n):
        return [(path, 1.) for path in self.sources][:n]
    def get_dependencies(self, path):
        return self.edges.get(path, [])
    def get_dependents(self, path):
        return []
    def generate_map(self, max_files=50):
        return "Repository overview\n" + "\n".join(self.sources)


class ControlledEmbedding:
    def __init__(self, anchor):
        self.anchor = anchor
    def find_relevant_files_scored(self, query, files, top_n):
        # No broad embedding fallback can accidentally rescue a broken BFS.
        return [(self.anchor, 1.)]


def test_real_graph_expands_selected_anchor_and_dual_edge_chain(tmp_path):
    files = []
    for name, source in {
        'a.py': 'from b import beta\ndef alpha():\n    return beta()\n',
        'b.py': 'from c import gamma\ndef beta():\n    return gamma()\n',
        'c.py': 'def gamma():\n    return 42\n',
    }.items():
        p = tmp_path / name; p.write_text(source); files.append(str(p))
    graph = RepoGraph(str(tmp_path)); graph.build_complete(files)
    manager = ContextManager(graph, ControlledEmbedding(tmp_path / 'a.py'), max_tokens=8000)
    found = manager._get_dependency_neighborhood([tmp_path / 'a.py'], {tmp_path / 'a.py'}, 3, None)
    assert {p.name for p, _ in found} == {'b.py', 'c.py'}
    result = manager.assemble_context_result('alpha', [], include_map=False)
    assert any(c['file'].endswith('/c.py') and c['selection_reason'] == 'neighborhood'
               for c in result.chunks)


def test_snapshot_source_is_used_even_when_disk_disagrees(tmp_path):
    graph = SnapshotGraph(tmp_path, {'a.py': 'def current():\n    return 1\n'})
    (tmp_path / 'a.py').write_text('def stale():\n    return 0\n')
    manager = ContextManager(graph, None, max_tokens=8000)
    result = manager.assemble_context_result('question', [tmp_path / 'a.py'], generation=17)
    assert 'current' in result.text and 'stale' not in result.text
    assert result.generation == 17
    assert result.chunks[0]['mode'] == 'full'
    chunk = result.chunks[0]
    assert result.text[chunk['start_char']:chunk['end_char']] == graph.sources[chunk['file']]


def test_unaccepted_explicit_file_does_not_fall_back_to_disk(tmp_path):
    (tmp_path / 'unknown.py').write_text('secret = 1')
    manager = ContextManager(SnapshotGraph(tmp_path, {}), None, max_tokens=8000)
    result = manager.assemble_context_result('q', [tmp_path / 'unknown.py'])
    assert not result.chunks
    assert result.counts['omission_reasons']['source_unavailable'] == 1
    assert 'secret = 1' not in result.text


def test_full_falls_back_to_complete_skeleton_and_exact_spans(tmp_path, monkeypatch):
    graph = SnapshotGraph(tmp_path, {'huge.py': 'def café():\n' + '    pass\n' * 6000})
    signature = 'def café():'
    def details(source, ext):
        return json.dumps({'text': signature + '\n    ...', 'chunks': [{
            'text': signature + '\n    ...', 'spans': [{
                'start_byte': 0, 'end_byte': len(signature.encode()), 'start_line': 1, 'end_line': 1}]}]})
    monkeypatch.setattr('atlas.context.semantic_engine.create_skeleton_details_json', details, raising=False)
    manager = ContextManager(graph, None, max_tokens=8000)
    result = manager.assemble_context_result('q', [tmp_path / 'huge.py'], include_map=False)
    assert len(result.chunks) == 1 and result.chunks[0]['mode'] == 'skeleton'
    chunk = result.chunks[0]
    assert result.text[chunk['start_char']:chunk['end_char']] == signature + '\n    ...'
    assert chunk['source_spans'][0]['end_byte'] == 12
    assert 'pass' not in result.text


def test_both_text_and_json_fit_transport_with_unicode(tmp_path):
    graph = SnapshotGraph(tmp_path, {f'f{i}.py': f'def f{i}():\n    return "🦋"\n' for i in range(40)})
    manager = ContextManager(graph, None, max_tokens=8000)
    result = manager.assemble_context_result('q', [], max_chars=3000, generation='g1')
    assert len(result.text) <= 3000
    assert len(json.dumps(result.to_dict())) <= 3000
    assert result.counts['tokens'] <= 8000 - manager.count_tokens('q') - 1000
    assert result.counts['omitted'] > 0
    for chunk in result.chunks:
        delivered = result.text[chunk['start_char']:chunk['end_char']]
        assert delivered.strip()
        if chunk['mode'] == 'full':
            assert delivered == graph.sources[chunk['file']]


def test_tiny_budget_returns_empty_source_and_honest_omissions(tmp_path):
    graph = SnapshotGraph(tmp_path, {'a.py': 'def alpha():\n    return 1\n'})
    manager = ContextManager(graph, None, max_tokens=100)
    result = manager.assemble_context_result('q', [])
    assert result.text == '' and not result.chunks
    assert result.counts['tokens'] == 0 and result.counts['omitted'] > 0


def test_adaptive_full_share_and_deduplicated_candidates(tmp_path):
    graph = SnapshotGraph(tmp_path, {'a.py': 'def alpha():\n    return 1\n'})
    manager = ContextManager(graph, None, max_tokens=8000)
    params = manager._compute_adaptive_params(6000, '')
    assert params.full_source_tokens == int(6000 * (.75 - 1 / 1500))
    assert params.full_source_tokens + params.skeleton_tokens == params.content_tokens
    result = manager.pack_ranked_context_result('q', [(tmp_path/'a.py', 'explicit'),
        (tmp_path/'a.py', 'anchor')], include_map=False)
    assert len(result.chunks) == 1 and result.chunks[0]['selection_reason'] == 'explicit'


def test_explicit_skeleton_precedes_lower_priority_full_sources(tmp_path, monkeypatch):
    sources = {'explicit.py': 'def explicit():\n' + '    body = 42\n' * 2000}
    sources.update({f'other{i}.py': f'def other{i}():\n' + '    x = 1\n' * 10 for i in range(15)})
    graph = SnapshotGraph(tmp_path, sources)
    doc = 'def explicit():\n    """' + 'API documentation. ' * 100 + '"""\n    ...'
    def details(source, ext):
        text = doc if 'def explicit' in source else 'def other():\n    ...'
        return json.dumps({'text': text, 'chunks': [{'text': text, 'spans': []}]})
    monkeypatch.setattr('atlas.context.semantic_engine.create_skeleton_details_json', details, raising=False)
    manager = ContextManager(graph, None, max_tokens=2200)
    candidates = [(tmp_path/'explicit.py', 'explicit')] + [(tmp_path/f'other{i}.py', 'architecture') for i in range(15)]
    result = manager.pack_ranked_context_result('q', candidates, include_map=False)
    assert result.chunks[0]['file'].endswith('/explicit.py')
    assert result.chunks[0]['mode'] == 'skeleton'


def test_full_source_line_ranges_use_lf_not_unicode_separators(tmp_path):
    graph = SnapshotGraph(tmp_path, {'a.py': 'x = "a\u2028b"\ny = 2\n'})
    manager = ContextManager(graph, None, max_tokens=8000)
    result = manager.assemble_context_result('q', [tmp_path/'a.py'], include_map=False)
    assert result.chunks[0]['source_spans'][0]['end_line'] == 2


def test_json_metadata_is_included_in_token_budget(tmp_path):
    graph = SnapshotGraph(tmp_path, {f'f{i}.py': f'def f{i}():\n    return {i}\n' for i in range(400)})
    manager = ContextManager(graph, None, max_tokens=8000)
    result = manager.assemble_context_result('q', [], include_map=False)
    available = 8000 - 1000 - manager.count_tokens('q')
    assert manager.count_tokens(json.dumps(result.to_dict())) <= available
    assert result.chunks and result.counts['omitted']


def test_accepted_graph_paths_do_not_resolve_against_live_disk(tmp_path, monkeypatch):
    graph = SnapshotGraph(tmp_path, {'a.py': 'def accepted():\n    return 1\n'})
    manager = ContextManager(graph, None, max_tokens=8000)
    def unexpected_resolve(self, *args, **kwargs):
        raise AssertionError('Accepted snapshot path touched live filesystem')
    monkeypatch.setattr(Path, 'resolve', unexpected_resolve)
    result = manager.assemble_context_result('q', [tmp_path/'a.py'], include_map=False)
    assert result.chunks and 'accepted' in result.text


def test_new_explicit_symlink_alias_still_resolves_to_accepted_id(tmp_path):
    path = tmp_path/'actual.py'; path.write_text('def actual():\n    return 1\n')
    alias = tmp_path/'alias.py'; alias.symlink_to(path)
    graph = SnapshotGraph(tmp_path, {'actual.py': path.read_text()})
    result = ContextManager(graph, None, max_tokens=8000).assemble_context_result('q', [alias], include_map=False)
    assert result.chunks[0]['file'] == str(path)
    assert result.chunks[0]['selection_reason'] == 'explicit'


def test_budget_rejection_does_not_parse_thousands_of_unreturnable_files(tmp_path):
    graph = SnapshotGraph(tmp_path, {f'f{i}.py': f'def f{i}():\n    return {i}\n' for i in range(1000)})
    calls=[]
    read=graph.get_source
    def counted(path): calls.append(path); return read(path)
    graph.get_source=counted
    manager=ContextManager(graph,None,max_tokens=2000)
    result=manager.assemble_context_result('q',[],include_map=False)
    assert result.chunks
    assert len(calls)<30
    assert result.counts['omitted']>900
    assert manager.count_tokens(json.dumps(result.to_dict()))<=999
