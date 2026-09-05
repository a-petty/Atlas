"""Benchmark accounting must follow delivered source, not retrieval labels."""
import json
from pathlib import Path
from types import SimpleNamespace

import pytest
from benchmarks.bench_contextbench import GoldBlock, RetrievedFile, compute_block_metrics
from benchmarks.bench_retrieval import BM25, frozen_manifest, retained_span_metrics, summarize
from benchmarks.bench_token_efficiency import parse_atlas_output


def test_skeleton_label_without_evidence_gets_no_block_credit():
    assert compute_block_metrics([GoldBlock('a.py', 10, 20, '')],
        [RetrievedFile('a.py', 'SKELETON')]) == (0, 0, 1)


def test_extended_context_is_counted_by_legacy_parser():
    output = '=' * 80 + '\nEXTENDED_CONTEXT (skeletons)\n' + '=' * 80 + '\n# /repo/a.py (SKELETON)\ndef foo(): ...'
    assert parse_atlas_output(output)['skeleton'] == ['/repo/a.py']


def test_gold_body_is_not_covered_by_delivered_signature(tmp_path):
    source = 'def foo():\n    body = 123\n    return body\n'
    path = tmp_path / 'a.py'; path.write_text(source)
    result = SimpleNamespace(text='def foo():\n    ...', chunks=[{'file': str(path),
        'start_char': 0, 'end_char': 18,
        'source_spans': [{'start_byte': 0, 'end_byte': 10}]}])
    metric = retained_span_metrics([GoldBlock('a.py', 2, 3, '')], result, tmp_path)
    assert metric['gold_byte_recall'] == 0
    assert metric['blocks_missed'] == 1


def test_duplicate_and_overlapping_spans_are_not_double_counted(tmp_path):
    path = tmp_path / 'a.py'; path.write_text('alpha\nbeta\n')
    result = SimpleNamespace(text='alpha\nalpha\n', chunks=[
        {'file': str(path), 'start_char': 0, 'end_char': 6, 'source_spans': [{'start_byte': 0, 'end_byte': 6}]},
        {'file': str(path), 'start_char': 6, 'end_char': 12, 'source_spans': [{'start_byte': 0, 'end_byte': 6}]}])
    metric = retained_span_metrics([GoldBlock('a.py', 1, 2, ''), GoldBlock('a.py', 1, 1, '')], result, tmp_path)
    assert metric['gold_bytes'] == 11
    assert metric['retained_gold_bytes'] == 6
    assert metric['blocks_fully_covered'] == 1
    assert metric['blocks_partially_covered'] == 1


def test_claimed_span_must_exist_in_delivered_text(tmp_path):
    path = tmp_path / 'a.py'; path.write_text('alpha\nbeta\n')
    result = SimpleNamespace(text='alpha\n...', chunks=[{'file': str(path), 'start_char': 0,
        'end_char': 9, 'source_spans': [{'start_byte': 6, 'end_byte': 11}]}])
    with pytest.raises(ValueError, match='absent'):
        retained_span_metrics([GoldBlock('a.py', 2, 2, '')], result, tmp_path)


def test_failure_remains_in_metric_denominator():
    rows = [{'strategy': 'atlas', 'budget': 8000, 'file_recall': 1., 'error': None},
            {'strategy': 'atlas', 'budget': 8000, 'error': 'checkout failed'}]
    metric = summarize(rows)[0]
    assert metric['tasks'] == 2 and metric['failed'] == 1
    assert metric['file_recall_mean'] == .5


def test_bm25_is_deterministic_and_matches_identifier_words():
    ranker = BM25([(Path('a.py'), 'def calculateBudget(): pass'), (Path('b.py'), 'graph nodes paths')])
    assert ranker.rank('calculate budget')[0] == Path('a.py')
    assert ranker.rank('unknown') == [Path('a.py'), Path('b.py')]


def test_frozen_sample_is_stratified_excludes_old_ids_and_pins_commits():
    rows = []
    for language in ('python', 'javascript', 'typescript', 'rust', 'go'):
        for i in range(15):
            rows.append(dict(instance_id=f'{language}{i}', language=language, repo='org/repo',
                             repo_url='https://github.com/org/repo.git', base_commit='a'*40))
    first = frozen_manifest(rows, 'dataset_hash', {'python1'})
    second = frozen_manifest(list(reversed(rows)), 'dataset_hash', {'python1'})
    assert first == second and len(first['tasks']) == 50
    assert 'python1' not in {row['instance_id'] for row in first['tasks']}
    assert all(row['base_commit'] == 'a'*40 for row in first['tasks'])


def _resume_fixture():
    from benchmarks.bench_retrieval import run_configuration, STRATEGIES, MODEL_NAME
    task = dict(instance_id='task1', repo='org/repo', language='python', base_commit='a'*40)
    manifest = {'dataset_sha256':'data', 'strategies':list(STRATEGIES), 'budgets':[8000,12000],
                'max_chars':60000, 'tasks':[task]}
    config = run_configuration(manifest,3,'actual-model-digest')
    rows = [{**task,'strategy':strategy,'budget':budget,'model':MODEL_NAME,
             'model_identity':'actual-model-digest','embedding_engine_key':'actual-model-digest','error':None}
            for strategy in STRATEGIES for budget in (8000,12000)]
    return {'manifest':manifest,'manifest_sha256':'manifest','run_configuration':config,'measurements':rows}


def test_resume_only_skips_complete_successful_tasks():
    from benchmarks.bench_retrieval import completed_tasks
    rows = _resume_fixture()['measurements']
    assert completed_tasks(rows,[8000,12000]) == {'task1'}
    assert completed_tasks(rows[:-1],[8000,12000]) == set()
    rows[-1]['error'] = 'transient failure'
    assert completed_tasks(rows,[8000,12000]) == set()


def test_resume_rejects_changed_manifest_model_strategy_or_duplicate_rows():
    import copy
    from benchmarks.bench_retrieval import validate_resume
    initial = _resume_fixture()
    validate_resume(initial,initial['manifest'],'manifest',initial['run_configuration'])
    for change in ('manifest','model','strategy','duplicate'):
        output=copy.deepcopy(initial)
        if change=='manifest': output['manifest_sha256']='different'
        elif change=='model': output['measurements'][0]['model_identity']='changed-weights'
        elif change=='strategy': output['measurements'][0]['strategy']='other'
        else: output['measurements'].append(output['measurements'][0])
        with pytest.raises(ValueError):
            validate_resume(output,initial['manifest'],'manifest',initial['run_configuration'])


def test_checkpoint_preserves_errors_atomically(tmp_path):
    from benchmarks.bench_retrieval import checkpoint
    output = _resume_fixture();output['measurements'][-1]['error']='model failed'
    destination=tmp_path/'results.json'
    checkpoint(output,destination)
    saved=json.loads(destination.read_text())
    assert saved['measurements'][-1]['error']=='model failed'
    assert not list(tmp_path.glob('*.tmp'))
    assert saved['partial'] is False


def test_checkout_fetches_only_exact_pinned_commit(tmp_path, monkeypatch):
    from benchmarks.bench_retrieval import pinned_checkout
    commit='a'*40;url='https://github.com/org/repo.git';calls=[]
    def git(command,**kwargs):
        calls.append(command)
        args=command[1:]
        if args[:2]==['init','--bare']: Path(args[2]).mkdir(parents=True)
        if args[:2]==['cat-file','-e']: return SimpleNamespace(returncode=1,stdout='',stderr='not fetched')
        if args[:2]==['remote','get-url']: return SimpleNamespace(returncode=0,stdout=url,stderr='')
        if args[:2]==['rev-parse','HEAD']: return SimpleNamespace(returncode=0,stdout=commit,stderr='')
        return SimpleNamespace(returncode=0,stdout='',stderr='')
    monkeypatch.setattr('benchmarks.bench_retrieval.subprocess.run',git)
    task={'repo':'org/repo','repo_url':url,'base_commit':commit}
    pinned_checkout(task,tmp_path)
    assert ['git','init','--bare',str(tmp_path/'org/repo.git')] in calls
    assert ['git','fetch','--depth=1','--no-tags','origin',commit] in calls
    assert not any('clone' in call for call in calls)


def test_cli_resume_skips_completed_task_and_checkpoints_unexpected_failure(tmp_path, monkeypatch):
    import sys
    from benchmarks import bench_retrieval as runner
    output = _resume_fixture()
    manifest = output['manifest']
    manifest['tasks'].append({**manifest['tasks'][0], 'instance_id':'task2'})
    manifest_path=tmp_path/'manifest.json';manifest_path.write_text(json.dumps(manifest))
    result_path=tmp_path/'result.json'
    monkeypatch.setattr(runner,'load_frozen_tasks',lambda dataset,pinned:pinned['tasks'])
    monkeypatch.setattr('atlas.embeddings.EmbeddingManager',lambda **kwargs:SimpleNamespace(engine_key='actual-model-digest'))
    calls=[]
    def successful(task, cache, budgets, repeats, model_identity=None):
        calls.append(task['instance_id'])
        return [{**row,'instance_id':task['instance_id']} for row in _resume_fixture()['measurements']]
    monkeypatch.setattr(runner,'evaluate',successful)
    base=['bench_retrieval','run','--dataset',str(tmp_path/'unused.parquet'),
          '--manifest',str(manifest_path),'--output',str(result_path)]
    monkeypatch.setattr(sys,'argv',base+['--limit','1']);runner.main()
    assert calls==['task1']
    def failed(task,*args,**kwargs): raise RuntimeError('injected failure')
    monkeypatch.setattr(runner,'evaluate',failed)
    monkeypatch.setattr(sys,'argv',base+['--resume'])
    with pytest.raises(RuntimeError,match='injected'):
        runner.main()
    checkpointed=json.loads(result_path.read_text())
    assert len(checkpointed['measurements'])==8
    assert checkpointed['status']=='failed'
    assert checkpointed['last_error']['task']=='task2'
    monkeypatch.setattr(runner,'evaluate',successful)
    runner.main()
    assert calls==['task1','task2']
    completed=json.loads(result_path.read_text())
    assert len(completed['measurements'])==16
    assert completed['partial'] is False and completed['status']=='finished'


def test_checkout_rejects_parent_path_components(tmp_path):
    from benchmarks.bench_retrieval import pinned_checkout
    with pytest.raises(ValueError,match='identity'):
        pinned_checkout({'repo':'../repo','repo_url':'https://github.com/org/repo.git','base_commit':'a'*40},tmp_path)


def test_resume_rejects_changed_implementation_fingerprint():
    import copy
    from benchmarks.bench_retrieval import validate_resume
    saved=_resume_fixture()
    changed=copy.deepcopy(saved['run_configuration'])
    changed['implementation_fingerprint']['sha256']='different extension or source'
    with pytest.raises(ValueError,match='implementation'):
        validate_resume(saved,saved['manifest'],'manifest',changed)


def test_implementation_fingerprint_contains_source_and_loaded_extension():
    from benchmarks.bench_retrieval import implementation_fingerprint
    identity=implementation_fingerprint()
    assert len(identity['sha256'])==64
    assert {'atlas/context.py','atlas/embeddings.py','atlas/semantic_engine.loaded-extension'} <= set(identity['files'])
    assert all(len(digest)==64 for digest in identity['files'].values())
    assert identity['dependencies']['numpy'] != 'unavailable'


def test_gold_line_ranges_use_physical_lf_bytes(tmp_path):
    raw = b"first\rstill_first\nsecond\n"
    path = tmp_path / "a.py"; path.write_bytes(raw)
    start = raw.index(b"second")
    result = SimpleNamespace(text="second\n", chunks=[{"file": str(path), "start_char": 0, "end_char": 7,
        "source_spans": [{"start_byte": start, "end_byte": len(raw)}]}])
    metric = retained_span_metrics([GoldBlock("a.py", 2, 2, "")], result, tmp_path)
    assert metric["gold_byte_recall"] == 1.
    assert metric["gold_bytes"] == 7


def test_gold_line_ranges_keep_unicode_separators_and_crlf_in_source(tmp_path):
    raw = 'first\u2028still_first\r\nsecond\r\n'.encode()
    path = tmp_path / 'a.py'; path.write_bytes(raw)
    start = raw.index(b'second')
    result = SimpleNamespace(text='second\r\n', chunks=[{'file': str(path),
        'start_char': 0, 'end_char': 8,
        'source_spans': [{'start_byte': start, 'end_byte': len(raw)}]}])
    metric = retained_span_metrics([GoldBlock('a.py', 2, 2, '')], result, tmp_path)
    assert metric['gold_byte_recall'] == 1.
    assert metric['gold_bytes'] == 8


def test_runner_uses_session_eligibility_and_accounts_for_actual_delivery(tmp_path, monkeypatch):
    """Exercise real capture, Rust parsing, packing and both MCP renderers."""
    from benchmarks import bench_retrieval as runner
    from atlas.context import ContextManager
    from atlas import mcp_server
    (tmp_path / 'src').mkdir()
    (tmp_path / 'node_modules').mkdir()
    for relative, source in {
        'src/main.py': 'from helper import reconcile\ndef invoice():\n    return reconcile()\n',
        'src/helper.py': 'def reconcile():\n    return 1\n',
        'src/broken.py': 'def incomplete(\n',
        'ignored.py': 'def ignored(): pass\n',
        'node_modules/vendor.js': 'export const vendor = 1;\n',
        '.atlasignore': 'ignored.py\n',
        '.atlas.toml': '[project]\nsource_roots = ["src"]\nlanguages = ["python"]\n',
    }.items():
        (tmp_path / relative).write_text(source)
    observed = {}
    class Embeddings:
        engine_key = 'deterministic-test-model'
        def __init__(self, repo_graph, **kwargs):
            self.graph = repo_graph
            observed['source_roots'] = repo_graph.get_statistics().source_roots
        def _get_embedding_text(self, path):
            return self.graph.get_source(str(path))
        def find_relevant_files_scored(self, query, files, top_n):
            assert self.generation_managed is True
            observed.setdefault('eligible_sets', []).append(set(files))
            return [(path, 1.) for path in sorted(files)[:top_n]]
    monkeypatch.setattr('atlas.embeddings.EmbeddingManager', Embeddings)
    monkeypatch.setattr(runner, 'pinned_checkout', lambda task, cache: tmp_path)
    elapsed = [0.]
    monkeypatch.setattr(runner, 'time', SimpleNamespace(perf_counter=lambda: elapsed[0]))
    actual_render, rendered = mcp_server._render, []
    def render(payload, response_format):
        value = actual_render(payload, response_format)
        rendered.append((response_format, payload['text'], value))
        elapsed[0] += .01
        return value
    monkeypatch.setattr(mcp_server, '_render', render)
    task = dict(instance_id='capture-fixture', repo='org/repo', language='python',
        base_commit='a'*40, problem_statement='invoice reconciliation', gold_context=[
            {'file': 'src/main.py', 'start_line': 1, 'end_line': 3},
            {'file': 'src/broken.py', 'start_line': 1, 'end_line': 1}])
    rows = runner.evaluate(task, tmp_path, [8000, 12000], repeats=1,
        model_identity='deterministic-test-model')
    assert len(rows) == 8
    assert not [row['error'] for row in rows if row['error']]
    assert len({row['eligible_files_sha256'] for row in rows}) == 1
    expected = {tmp_path / 'src/main.py', tmp_path / 'src/helper.py'}
    assert all(files == expected for files in observed['eligible_sets'])
    assert any(Path(path).name == 'src' for path in observed['source_roots'])
    tokenizer = ContextManager(None, None)
    for index, row in enumerate(rows):
        # Two iterations, each of which renders JSON then text.
        response_format, source_text, delivered_text = rendered[index * 4 + 3]
        delivered_json = rendered[index * 4 + 2][2]
        assert response_format == 'text' and '\nCOVERAGE: ' in delivered_text
        assert row['eligible_files'] == 2
        assert row['coverage']['eligible'] == 3 and row['coverage']['invalid_count'] == 1
        assert row['incomplete'] is True and row['invalid_gold_blocks'] == []
        assert row['file_recall'] == .5
        assert row['tokens'] == tokenizer.count_tokens(delivered_text)
        assert row['tokens'] > tokenizer.count_tokens(source_text)
        assert row['json_tokens'] == tokenizer.count_tokens(delivered_json)
        assert max(row['tokens'], row['json_tokens']) <= row['budget']
        assert max(row['characters'], row['json_characters']) <= 60000
        assert row['warm_assembly_ms'] == [0.]
        assert row['warm_latency_ms'] == pytest.approx([20.])
