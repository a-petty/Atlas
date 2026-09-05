#!/usr/bin/env python3
"""Benchmark an isolated pinned public checkout through Atlas's normal session.

Source is read from --repository, which must be an isolated editable copy.
The full original commit is required provenance. No source is imported/executed,
no paid models are called, and local embedding-model downloads stay disabled.
"""
import argparse
import ast
import collections
import hashlib
import json
import os
from pathlib import Path
import time

try:
    from benchmarks.bench_scale import TARGETS, EXTENSIONS, cold_request, persist, profile_evidence, summarize
except ModuleNotFoundError:
    from bench_scale import TARGETS, EXTENSIONS, cold_request, persist, profile_evidence, summarize

QUERIES = [
    'How does Django initialize application configuration and load models?',
    'How are model fields and database columns discovered?',
    'Where are incoming requests dispatched through middleware?',
    'How are template variables and filters resolved?',
    'How do form fields validate incoming values?',
    'How are database migration dependencies ordered?',
    'Where does Django find and serve static files?',
    'How does model serialization handle related objects?',
    'How are cache keys generated and invalidated?',
    'How do lazy objects defer evaluating wrapped values?',
]


def insertion_edit(original, iteration):
    """Insert one unused local assignment after a function's docstring."""
    text = original.decode('utf-8')
    tree = ast.parse(text)
    function = next(node for node in ast.walk(tree) if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.body)
    first = function.body[0]
    has_doc = isinstance(first, ast.Expr) and isinstance(first.value, ast.Constant) and isinstance(first.value.value, str)
    at = first.end_lineno if has_doc else first.lineno - 1
    lines = text.splitlines(keepends=True)
    indent = ' ' * first.col_offset
    newline = '\r\n' if any(line.endswith('\r\n') for line in lines[:5]) else '\n'
    lines.insert(at, indent + f'_atlas_scale_iteration = {iteration}' + newline)
    changed = ''.join(lines)
    ast.parse(changed)
    return changed.encode('utf-8')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repository', type=Path, required=True)
    parser.add_argument('--repo-name', required=True)
    parser.add_argument('--commit', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--cache', type=Path, required=True)
    parser.add_argument('--probe-file', default='django/apps/registry.py')
    parser.add_argument('--edit-file', default='django/utils/functional.py')
    parser.add_argument('--samples', type=int, default=10)
    parser.add_argument('--cold-timeout', type=float, default=240)
    args = parser.parse_args()
    if args.samples < 10: parser.error('At least10samples are required')
    if len(args.commit) != 40 or any(c not in '0123456789abcdef' for c in args.commit):
        parser.error('--commit must be the full lower-case40-character SHA')
    from atlas.session import RepositorySession, IGNORED_DIRS
    from atlas.semantic_engine import scan_repository
    root = args.repository.resolve()
    paths = sorted(Path(p) for p in scan_repository(str(root), ignored_dirs=sorted(IGNORED_DIRS)) if Path(p).suffix.lower()[1:] in EXTENSIONS.values() or Path(p).suffix == '.pyi')
    source_digest = hashlib.sha256()
    source_bytes = 0
    source_manifest = []
    for path in paths:
        relative = path.relative_to(root).as_posix()
        content = path.read_bytes(); content_hash = hashlib.sha256(content).hexdigest()
        source_digest.update(relative.encode() + b'\0' + bytes.fromhex(content_hash)); source_bytes += len(content)
        source_manifest.append({'path': relative, 'bytes': len(content), 'sha256': content_hash})
    persist(args.output.with_suffix('.manifest.json'), {'repo': args.repo_name, 'commit': args.commit, 'files': source_manifest})
    os.environ['HF_HUB_OFFLINE'] = '1'; os.environ['ATLAS_CACHE_DIR'] = str(args.cache.resolve())
    result = {'schema_version': 1, 'repository': {'name': args.repo_name, 'commit': args.commit, 'path': str(root),
        'eligible_files': len(paths), 'source_bytes': source_bytes, 'source_sha256': source_digest.hexdigest(),
        'extensions': dict(collections.Counter(p.suffix for p in paths))}, 'build': profile_evidence('release'),
        'started_unix': time.time(), 'cold': {}, 'warm': {}, 'memory': [], 'errors': [], 'complete': False,
        'cache': {'path': str(args.cache.resolve()), 'entries_before': len(list(args.cache.rglob('*.npz')))},
        'query_set': QUERIES, 'probe_file': args.probe_file, 'edit_file': args.edit_file}
    checkpoint = lambda: persist(args.output, result)
    session = RepositorySession(root, timeout=args.cold_timeout)
    def memory(label):
        runtime = session.request('status')['runtime']
        result['memory'].append({'label': label, 'peak_rss_bytes': runtime['peak_rss_bytes'], 'pid': runtime['pid']})
    checkpoint()
    try:
        status, duration, timeline = cold_request(session, 'status')
        result['cold']['graph'] = {'seconds': duration, 'progress_timeline': timeline}
        result['coverage'] = status['coverage']
        result['statistics'] = status['statistics']
        memory('after_cold_graph'); checkpoint()
        print(json.dumps({'event': 'public_graph_complete', 'seconds': duration, 'coverage': status['coverage']}), flush=True)
        for operation, params in [('dependencies', {'file_path': args.probe_file}), ('map', {'limit': 50})]:
            values = []
            for _ in range(args.samples):
                started = time.perf_counter(); session.request(operation, **params); values.append(time.perf_counter()-started)
            result['warm'][operation] = summarize(values, TARGETS[operation]); checkpoint()
            print(json.dumps({'event': 'public_warm_complete', 'operation': operation, 'p95_seconds': result['warm'][operation]['p95_seconds']}), flush=True)
        _, duration, timeline = cold_request(session, 'search', query=QUERIES[0], limit=10)
        result['cold']['semantic_preparation'] = {'seconds': duration, 'progress_timeline': timeline,
            'includes': 'local model initialization and embedding every accepted source file before returning first result'}
        memory('after_semantic_preparation'); checkpoint()
        print(json.dumps({'event': 'public_semantic_preparation_complete', 'seconds': duration}), flush=True)
        session.timeout = 45
        for operation in ('search', 'context'):
            values = []
            for i in range(args.samples):
                params = {'query': QUERIES[i % len(QUERIES)]}
                if operation == 'search': params['limit'] = 10
                else: params.update(max_tokens=12000, files_in_scope=[args.probe_file])
                started = time.perf_counter(); answer = session.request(operation, **params); values.append(time.perf_counter()-started)
                if operation == 'context': assert len(json.dumps(answer)) <= 60000
            result['warm'][operation] = summarize(values, TARGETS[operation]); memory('after_warm_' + operation); checkpoint()
            print(json.dumps({'event': 'public_warm_complete', 'operation': operation, 'p95_seconds': result['warm'][operation]['p95_seconds']}), flush=True)
        path = root / args.edit_file; original = path.read_bytes(); values = []
        try:
            for i in range(args.samples):
                changed = insertion_edit(original, i)
                started = time.perf_counter(); path.write_bytes(changed)
                answer = session.request('context', query=QUERIES[0], max_tokens=12000, files_in_scope=[args.edit_file])
                values.append(time.perf_counter()-started)
                assert session.request('graph', method='get_source', args=[str(path)]).encode() == changed
                memory('after_edit_' + str(i+1))
        finally:
            path.write_bytes(original); session.request('status')
        result['warm']['edit_next_answer'] = summarize(values, TARGETS['edit_next_answer'])
        result['edit_next_answer_operation'] = 'context'
        memory('after_restore')
        result['complete'] = True
        result['meets_measured_targets'] = all(item['meets_target'] for item in result['warm'].values())
    except Exception as exc:
        result['errors'].append({'type': type(exc).__name__, 'message': str(exc), 'last_progress': session.progress})
    finally:
        session.close()
        result['elapsed_seconds'] = time.time()-result['started_unix']
        result['cache']['entries_after'] = len(list(args.cache.rglob('*.npz')))
        result['memory_note'] = 'Worker peakRSS observations; finite edit-cycle sampling does not prove asymptotic memory bounds.'
        checkpoint()
    print(json.dumps({'event': 'public_benchmark_complete', 'complete': result['complete'], 'errors': result['errors']}), flush=True)
    if not result['complete']: raise SystemExit(1)


if __name__ == '__main__': main()
