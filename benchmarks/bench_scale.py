#!/usr/bin/env python3
"""Reproducible local Atlas scale benchmark through RepositorySession.

Example (run from the repository using its freshly built extension):
  uv run python benchmarks/bench_scale.py --fixture work/scale-fixture \
    --output work/scale-results.json --build-profile release --samples 10

No paid inference. The existing local FastEmbed model is used with downloads
blocked by default. Cold timeout and unavailable semantic results are recorded
as incomplete, never substituted with a smaller fixture or mocked embeddings.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, TimeoutError as FutureTimeout
import hashlib
import json
import importlib.metadata
import math
import os
from pathlib import Path
import platform
import random
import statistics
import subprocess
import sys
import time

LANGUAGES = ('python', 'javascript', 'typescript', 'rust', 'go')
EXTENSIONS = dict(zip(LANGUAGES, ('py', 'js', 'ts', 'rs', 'go')))
TOPICS = ('invoice reconciliation', 'customer authentication', 'inventory reservation',
          'batch validation', 'account reporting', 'event routing', 'document indexing',
          'request scheduling', 'budget allocation', 'transaction approval')
FIXTURE_VERSION = 1
DEFAULT_SEED = 20260905
TARGETS = {'dependencies': .5, 'map': .5, 'search': 2., 'context': 2., 'edit_next_answer': 2.}


def source_file(language, index, total, seed):
    rng = random.Random((seed << 16) + index)
    constant = rng.randrange(1, 9999)
    topic = TOPICS[index % len(TOPICS)]
    name, next_name = f'process_{index:04d}', f'process_{index+1:04d}'
    following = index + 1 < total
    if language == 'python':
        imports = f'from module_{index+1:04d} import {next_name}\n' if following else ''
        body = f'{next_name}(value)' if following else 'value'
        return (f'"""{topic.title()} public service API."""\n{imports}\nLIMIT = {constant}\n\n'
                f'def {name}(value: int) -> int:\n'
                f'    """Validate {topic} input and return the normalized result."""\n'
                f'    normalized = max(0, value)\n    checked = normalized + LIMIT\n'
                f'    checked = checked - LIMIT\n    return {body}\n')
    if language in ('javascript', 'typescript'):
        ext = EXTENSIONS[language]
        imports = f"import {{ {next_name} }} from './module_{index+1:04d}.{ext}';\n" if following else ''
        annotation = ': number' if language == 'typescript' else ''
        body = f'{next_name}(value)' if following else 'value'
        return (f'/** {topic.title()} public service API. */\n{imports}'
                f'export const LIMIT{annotation} = {constant};\n'
                f'/** Validate {topic} input and return the normalized result. */\n'
                f'export function {name}(value{annotation}){annotation} {{\n'
                '  const normalized = Math.max(0, value);\n'
                '  const checked = normalized + LIMIT;\n'
                f'  return {body};\n}}\n')
    if language == 'rust':
        imports = f'use crate::module_{index+1:04d}::{next_name};\n' if following else ''
        body = f'{next_name}(value)' if following else 'value'
        return (f'//! {topic.title()} public service API.\n{imports}'
                f'pub const LIMIT: i64 = {constant};\n'
                f'/// Validate {topic} input and return the normalized result.\n'
                f'pub fn {name}(value: i64) -> i64 {{\n'
                '    let normalized = value.max(0);\n'
                '    let checked = normalized + LIMIT;\n'
                f'    {body}\n}}\n')
    body = f'{next_name}(value)' if following else 'value'
    return (f'// Package api implements {topic}.\npackage api\n\n'
            f'const Limit{index:04d} = {constant}\n'
            f'// {name} validates {topic} input and returns the normalized result.\n'
            f'func {name}(value int) int {{\n'
            f'    normalized := value + Limit{index:04d}\n'
            '    _ = normalized\n'
            f'    return {body}\n}}\n')


def create_fixture(root, file_count=10000, seed=DEFAULT_SEED):
    """Idempotent fixture preparation; refuse to overwrite an unrelated folder."""
    root = Path(root).resolve()
    if file_count < 5 or file_count % 5:
        raise ValueError('file_count must be a positive multiple of five')
    marker = root / '.atlas-scale-fixture.json'
    if root.exists() and any(root.iterdir()) and not marker.exists():
        raise ValueError(f'Refusing nonempty directory without fixture marker: {root}')
    root.mkdir(parents=True, exist_ok=True)
    # Mark ownership first so interrupted generation is recoverable.
    marker.write_text(json.dumps({'version': FIXTURE_VERSION, 'seed': seed, 'file_count': file_count, 'state': 'preparing'}))
    files, digest = [], hashlib.sha256()
    each = file_count // 5
    for language in LANGUAGES:
        folder = root / language
        folder.mkdir(exist_ok=True)
        for index in range(each):
            relative = f'{language}/module_{index:04d}.{EXTENSIONS[language]}'
            content = source_file(language, index, each, seed).encode('utf-8')
            path = root / relative
            if not path.exists() or path.read_bytes() != content:
                path.write_bytes(content)
            content_hash = hashlib.sha256(content).hexdigest()
            digest.update(relative.encode() + b'\0' + bytes.fromhex(content_hash))
            files.append({'path': relative, 'bytes': len(content), 'sha256': content_hash})
    expected = {item['path'] for item in files}
    actual = {p.relative_to(root).as_posix() for p in root.rglob('*') if p.suffix[1:] in EXTENSIONS.values()}
    if actual != expected:
        raise ValueError('Fixture contains unexpected source files; use a fresh fixture directory')
    config = '[project]\nsource_roots = ["python"]\nlanguages = ["python", "typescript"]\n'
    (root / '.atlas.toml').write_text(config)
    manifest = {'version': FIXTURE_VERSION, 'seed': seed, 'file_count': file_count,
                'files_by_language': {name: each for name in LANGUAGES},
                'source_bytes': sum(item['bytes'] for item in files),
                'fixture_sha256': digest.hexdigest(),
                'configuration_sha256': hashlib.sha256(config.encode()).hexdigest(),
                'resolver_scope': 'Python and JS/TS only; Rust/Go remain parser/skeleton coverage',
                'files': files}
    marker.write_text(json.dumps(manifest, indent=2) + '\n')
    return manifest


def percentile95(values):
    if not values: return None
    return sorted(values)[max(0, math.ceil(.95 * len(values)) - 1)]


def summarize(values, target=None):
    result = {'samples_seconds': values, 'sample_count': len(values),
              'median_seconds': statistics.median(values) if values else None,
              'p95_seconds': percentile95(values), 'maximum_seconds': max(values) if values else None}
    if target is not None:
        result.update(target_seconds=target, meets_target=bool(values) and percentile95(values) <= target)
    return result


def command_output(args):
    try:
        return subprocess.check_output(args, text=True, stderr=subprocess.DEVNULL).strip()
    except (OSError, subprocess.CalledProcessError):
        return None


def persist(path, result):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + '.tmp')
    temporary.write_text(json.dumps(result, indent=2, default=str) + '\n')
    temporary.replace(path)


def cold_request(session, operation, **params):
    """Sample public progress while one exact request timer covers all work."""
    timeline = []
    started = time.perf_counter()
    with ThreadPoolExecutor(max_workers=1) as pool:
        future = pool.submit(session.request, operation, **params)
        previous_phase = None
        previous_log = started
        while True:
            try:
                value = future.result(timeout=.1)
                return value, time.perf_counter() - started, timeline
            except FutureTimeout:
                # concurrent.futures.TimeoutError aliases built-in TimeoutError;
                # distinguish a completed request failure from our poll timeout.
                if future.done():
                    future.result()
                progress = dict(session.progress)
                now = time.perf_counter()
                if progress.get('phase') != previous_phase or now - previous_log >= 10:
                    event = {'seconds': now - started, **progress}
                    timeline.append(event)
                    print(json.dumps({'event': 'cold_progress', **event}), flush=True)
                    previous_phase = progress.get('phase')
                    previous_log = now


def profile_evidence(label):
    from atlas import semantic_engine
    path = Path(semantic_engine.__file__)
    return {'label': label, 'extension_path': str(path),
            'extension_sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'benchmark_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            'python_source_sha256': {str(p.relative_to(path.parent)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(path.parent.rglob('*.py'))},
            'embedding_model': {'name': 'BAAI/bge-small-en-v1.5', 'fastembed_version': importlib.metadata.version('fastembed')},
            'git_commit': command_output(['git', 'rev-parse', 'HEAD']),
            'tracked_diff_sha256': hashlib.sha256((command_output(['git', 'diff']) or '').encode()).hexdigest(),
            'python': sys.version, 'platform': platform.platform(),
            'machine': platform.machine(), 'cpu': command_output(['sysctl', '-n', 'machdep.cpu.brand_string']),
            'logical_cpus': os.cpu_count(), 'physical_memory_bytes': command_output(['sysctl', '-n', 'hw.memsize'])}


def run(args, manifest):
    from atlas.session import RepositorySession
    os.environ.setdefault('HF_HUB_OFFLINE', '1')
    os.environ['ATLAS_CACHE_DIR'] = str(args.cache.resolve())
    cache_before = list(args.cache.rglob('*.npz')) if args.cache.exists() else []
    result = {'schema_version': 1, 'started_unix': time.time(), 'build': profile_evidence(args.build_profile),
              'fixture': {key: value for key, value in manifest.items() if key != 'files'},
              'fixture_path': str(args.fixture.resolve()), 'samples_requested': args.samples,
              'cache': {'path': str(args.cache.resolve()), 'entries_before': len(cache_before),
                        'content_cache_cold': not cache_before,
                        'model_weights': 'existing local cache; downloads blocked by HF_HUB_OFFLINE'},
              'cold': {}, 'warm': {}, 'memory': [], 'errors': [], 'complete': False}
    session = RepositorySession(args.fixture, timeout=args.cold_timeout)
    checkpoint = lambda: persist(args.output, result)
    checkpoint()
    def memory(label):
        runtime = session.request('status').get('runtime', {})
        result['memory'].append({'label': label, 'peak_rss_bytes': runtime.get('peak_rss_bytes'), 'pid': runtime.get('pid')})
    try:
        status, duration, timeline = cold_request(session, 'status')
        result['cold']['graph'] = {'seconds': duration, 'progress_timeline': timeline,
                                   'indexed': status['coverage']['indexed'], 'generation': status['generation']}
        assert status['coverage']['indexed'] == manifest['file_count'], status
        assert status['coverage']['invalid_count'] == 0, status
        result['coverage'] = status['coverage']
        memory('after_cold_graph')
        checkpoint()
        print(json.dumps({'event': 'cold_graph_complete', 'seconds': duration, 'indexed': status['coverage']['indexed']}), flush=True)
        for operation, params in [('dependencies', {'file_path': 'python/module_0000.py'}), ('map', {'limit': 50})]:
            values = []
            for _ in range(args.samples):
                started = time.perf_counter(); page = session.request(operation, **params)
                values.append(time.perf_counter() - started)
                assert page['generation'] == status['generation']
            result['warm'][operation] = summarize(values, TARGETS[operation])
            print(json.dumps({'event': 'warm_complete', 'operation': operation, **result['warm'][operation]}), flush=True)
            checkpoint()
        if not args.skip_semantic:
            old_pid = session._process.pid
            try:
                _, duration, timeline = cold_request(session, 'search', query=TOPICS[0], limit=10)
                sampled_phases = {}
                for index, event in enumerate(timeline):
                    end = timeline[index+1]['seconds'] if index+1 < len(timeline) else duration
                    sampled_phases[event.get('phase', 'unknown')] = sampled_phases.get(event.get('phase', 'unknown'), 0.) + end - event['seconds']
                result['cold']['semantic_preparation'] = {'seconds': duration, 'progress_timeline': timeline, 'sampled_phase_seconds': sampled_phases,
                    'includes': 'model initialization, tokenization, all eligible file embeddings, persistent writes, matrix assembly and first query',
                    'phase_timing_resolution_seconds': .1}
                memory('after_semantic_preparation')
                checkpoint()
                print(json.dumps({'event': 'semantic_preparation_complete', 'seconds': duration}), flush=True)
            except Exception as exc:
                result['errors'].append({'stage': 'cold_semantic_preparation', 'type': type(exc).__name__, 'message': str(exc), 'last_progress': session.progress})
                result['cold']['semantic_preparation'] = {'complete': False, 'timeout_seconds': args.cold_timeout}
                # Verify retry recovers an accepted full-size graph; do not hide
                # incomplete cold preparation by testing a smaller repository.
                session.timeout = args.cold_timeout
                retry = session.request('status')
                result['restart_after_cold_failure'] = {'indexed': retry['coverage']['indexed'],
                    'new_pid': session._process.pid, 'old_pid': old_pid,
                    'different_process': session._process.pid != old_pid}
                checkpoint()
            else:
                session.timeout = args.warm_timeout
                for operation in ('search', 'context'):
                    values, delivered = [], []
                    for index in range(args.samples):
                        params = {'query': TOPICS[index % len(TOPICS)]}
                        if operation == 'search': params['limit'] = 10
                        else: params.update(max_tokens=12000, files_in_scope=['python/module_0000.py'])
                        started = time.perf_counter(); page = session.request(operation, **params)
                        values.append(time.perf_counter() - started)
                        delivered.append(page.get('counts', {'items': len(page['items'])} if 'items' in page else {}))
                    result['warm'][operation] = {**summarize(values, TARGETS[operation]), 'delivered': delivered}
                    memory('after_warm_' + operation)
                    checkpoint()
                    print(json.dumps({'event': 'warm_complete', 'operation': operation, 'p95_seconds': percentile95(values)}), flush=True)
        # An ordinary body edit changes accepted source while keeping its public
        # API stable. Time includes filesystem write plus the very next answer.
        edit_relative = f"python/module_{manifest['files_by_language']['python']//2:04d}.py"
        path = args.fixture / edit_relative
        semantic_ready = 'context' in result['warm']
        result['edit_next_answer_operation'] = 'context' if semantic_ready else 'dependencies'
        result['edit_fixture_file'] = edit_relative
        original = path.read_bytes()
        edit_values = []
        try:
            for index in range(args.samples):
                changed = original.replace(b'normalized = max(0, value)', f'normalized = max({index+1}, value)'.encode())
                started = time.perf_counter()
                path.write_bytes(changed)
                answer = (session.request('context', query=TOPICS[0], files_in_scope=[edit_relative], max_tokens=12000) if semantic_ready
                          else session.request('dependencies', file_path=edit_relative))
                edit_values.append(time.perf_counter() - started)
                assert answer['generation'] != status['generation']
                status = answer
                accepted = session.request('graph', method='get_source', args=[str(path.resolve())])
                assert accepted.encode() == changed
                memory('after_edit_' + str(index + 1))
        finally:
            path.write_bytes(original)
            session.request('status')
        result['warm']['edit_next_answer'] = summarize(edit_values, TARGETS['edit_next_answer'])
        memory('after_edit_series_and_restore')
        memory_values = [entry['peak_rss_bytes'] for entry in result['memory'] if entry['peak_rss_bytes'] is not None]
        result['memory_observation'] = {'maximum_peak_rss_bytes': max(memory_values) if memory_values else None,
            'note': 'Worker resource.ru_maxrss peak observations outside request timers; these are not current RSS. Finite repeated-edit observation does not prove asymptotic memory boundedness.'}
        result['complete'] = not result['errors'] and (args.skip_semantic or {'search', 'context'} <= result['warm'].keys())
        result['semantic_skipped'] = args.skip_semantic
        result['meets_measured_targets'] = result['complete'] and all(item['meets_target'] for item in result['warm'].values())
    except Exception as exc:
        result['errors'].append({'stage': 'benchmark', 'type': type(exc).__name__, 'message': str(exc), 'last_progress': session.progress})
    finally:
        session.close()
        result['elapsed_seconds'] = time.time() - result['started_unix']
        result['cache']['entries_after'] = len(list(args.cache.rglob('*.npz'))) if args.cache.exists() else 0
        checkpoint()
    print(json.dumps({'event': 'benchmark_complete', 'complete': result['complete'], 'output': str(args.output), 'errors': result['errors']}), flush=True)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fixture', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--cache', type=Path, default=Path('work/scale-embedding-cache'))
    parser.add_argument('--files', type=int, default=10000)
    parser.add_argument('--seed', type=int, default=DEFAULT_SEED)
    parser.add_argument('--samples', type=int, default=10)
    parser.add_argument('--cold-timeout', type=float, default=180)
    parser.add_argument('--warm-timeout', type=float, default=30)
    parser.add_argument('--build-profile', choices=('debug', 'release'), required=True)
    parser.add_argument('--skip-semantic', action='store_true')
    parser.add_argument('--prepare-only', action='store_true')
    args = parser.parse_args()
    if args.samples < 10: parser.error('At least 10 samples are required for reported p95 values')
    manifest = create_fixture(args.fixture, args.files, args.seed)
    if args.prepare_only:
        persist(args.output, manifest)
        print(json.dumps({key: value for key, value in manifest.items() if key != 'files'}), flush=True)
        return
    result = run(args, manifest)
    if not result['complete']: raise SystemExit(1)


if __name__ == '__main__':
    main()
