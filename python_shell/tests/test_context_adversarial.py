"""Desired-behavior regressions reproduced independently on 2026-09-05."""
import json
from pathlib import Path
from types import SimpleNamespace

from atlas.context import ContextManager
from atlas.semantic_engine import RepoGraph


class SnapshotGraph:
    def __init__(self, root, sources):
        self.project_root = str(root.resolve())
        self.sources = {str(root.resolve() / path): source for path, source in sources.items()}
    def get_source(self, path): return self.sources.get(path)
    def get_statistics(self): return SimpleNamespace(node_count=len(self.sources), edge_count=0)
    def get_top_ranked_files(self, n): return [(path, 1.) for path in self.sources][:n]
    def get_dependencies(self, path): return []
    def get_dependents(self, path): return []
    def generate_map(self, max_files=50): return ''


def test_explicit_api_cannot_be_displaced_by_lower_priority_full_files(tmp_path, monkeypatch):
    def details(source, ext):
        text = (source.split('# BODY')[0] if source.startswith('def explicit') else source.splitlines()[0] + '\n') + '    ...\n'
        return json.dumps({'text': text, 'chunks': [{'text': text, 'spans': []}]})
    monkeypatch.setattr('atlas.context.semantic_engine.create_skeleton_details_json', details, raising=False)
    explicit = 'def explicit():\n    """' + 'Public contract ' * 190 + '"""\n# BODY\n' + '    pass\n' * 2000
    filler = lambda i: f'def filler_{i}():\n' + '    value = 42\n' * 7 + '    return value\n'
    for count in [0, 15]:
        graph = SnapshotGraph(tmp_path, {'explicit.py': explicit, **{f'f{i}.py': filler(i) for i in range(count)}})
        manager = ContextManager(graph, None, max_tokens=2000)
        result = manager.assemble_context_result('', [tmp_path / 'explicit.py'], include_map=False)
        assert any(c['file'].endswith('/explicit.py') for c in result.chunks), result.to_dict()


def test_full_source_spans_use_physical_lf_lines_like_treesitter(tmp_path):
    graph = SnapshotGraph(tmp_path, {'lines.py': 'x = "a\u2028b"\ny = 2\n'})
    result = ContextManager(graph, None, max_tokens=8000).assemble_context_result('', [], include_map=False)
    assert result.chunks[0]['source_spans'][0]['end_line'] == 2


def test_alias_import_usage_allows_second_hop_expansion(tmp_path):
    files = []
    sources = {'a.py': 'from b import beta\ndef alpha():\n    return beta()\n',
               'b.py': 'from c import gamma as renamed\ndef beta():\n    return renamed()\n',
               'c.py': 'def gamma():\n    return 42\n'}
    for name, source in sources.items():
        p = tmp_path / name
        p.write_text(source)
        files.append(str(p))
    graph = RepoGraph(str(tmp_path))
    graph.build_complete(files)
    edges = graph.get_dependencies(str(tmp_path / 'b.py'))
    assert (str(tmp_path / 'c.py'), 'SymbolUsage') in edges
    manager = ContextManager(graph, None, max_tokens=8000)
    found = manager._get_dependency_neighborhood([tmp_path / 'a.py'], set(), 3, None)
    assert {p.name for p, _ in found} == {'b.py', 'c.py'}


def test_importing_unrelated_name_does_not_turn_parameter_call_into_symbol_usage(tmp_path):
    sources = {'a.py': 'from b import unused\ndef run(foo):\n    return foo()\n',
               'b.py': 'unused = 3\ndef foo():\n    return 1\n'}
    files = []
    for name, source in sources.items():
        p = tmp_path / name
        p.write_text(source)
        files.append(str(p))
    graph = RepoGraph(str(tmp_path))
    graph.build_complete(files)
    assert (str(tmp_path / 'b.py'), 'SymbolUsage') not in graph.get_dependencies(str(tmp_path / 'a.py'))
