"""Frozen ContextBench comparison using the actual delivered-source manifest.

Prepare: python -m benchmarks.bench_retrieval freeze --dataset FILE --output MANIFEST
Run: python -m benchmarks.bench_retrieval run --dataset FILE --manifest MANIFEST \
     --repo-cache /tmp/atlas-contextbench --output /tmp/atlas-retrieval.json

No model API calls are made. Repository acquisition uses public Git clones.
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import hashlib
import importlib
import importlib.metadata
import json
import math
import os
import platform
from pathlib import Path
import random
import re
import subprocess
import time
from datetime import datetime, timezone

from benchmarks.bench_contextbench import parse_gold_context, compute_file_metrics

LANGUAGES = ('python', 'javascript', 'typescript', 'rust', 'go')
STRATEGIES = ('bm25', 'flat_embedding', 'atlas_no_expansion', 'atlas')
DATASET_REVISION = 'c2855792b006af41c67202d33883fb9d46362853'
SEED = 20260905
MODEL_NAME = 'BAAI/bge-small-en-v1.5'
RUNNER_SCHEMA_VERSION = 2
DEFAULT_MANIFEST = Path(__file__).parent / 'manifests/contextbench_50_v1.json'


def dataset_rows(dataset: Path):
    import pyarrow.parquet as pq
    return pq.read_table(dataset).to_pylist()


def frozen_manifest(rows, dataset_sha256, excluded, per_language=10, seed=SEED):
    rng = random.Random(seed)
    selected = []
    for language in LANGUAGES:
        pool = sorted((r for r in rows if r['language'].lower() == language
                       and r['instance_id'] not in excluded), key=lambda r: r['instance_id'])
        if len(pool) < per_language:
            raise ValueError(f'Not enough previously unused {language} tasks')
        for task in sorted(rng.sample(pool, per_language), key=lambda r: r['instance_id']):
            selected.append({key: task[key] for key in
                            ('instance_id', 'repo', 'repo_url', 'base_commit', 'language')})
    return {'schema_version': 1, 'dataset': 'Contextbench/ContextBench',
            'dataset_revision': DATASET_REVISION, 'dataset_sha256': dataset_sha256,
            'seed': seed, 'per_language': per_language,
            'excluded_instance_ids': sorted(excluded), 'tasks': selected,
            'budgets': [8000, 12000], 'max_chars': 60000,
            'strategies': list(STRATEGIES), 'ranking_tuning_after_freeze': False}


def load_frozen_tasks(dataset, manifest):
    actual_hash = hashlib.sha256(dataset.read_bytes()).hexdigest()
    if actual_hash != manifest['dataset_sha256']:
        raise ValueError('Dataset hash differs from frozen manifest')
    rows = {r['instance_id']: r for r in dataset_rows(dataset)}
    tasks = []
    for pinned in manifest['tasks']:
        row = rows[pinned['instance_id']]
        if any(row[key] != value for key, value in pinned.items()):
            raise ValueError(f'Dataset row differs from manifest: {pinned["instance_id"]}')
        tasks.append(row)
    return tasks


def pinned_checkout(task, cache):
    """Dedicated detached worktrees; never delete or overwrite an existing checkout."""
    repo = task['repo']; commit = task['base_commit']
    if (not re.fullmatch(r'[\w.-]+/[\w.-]+', repo) or any(part in {'.', '..'} for part in repo.split('/'))
            or not re.fullmatch(r'[0-9a-f]{40}', commit)):
        raise ValueError('Invalid pinned repository identity')
    url = task.get('repo_url') or f'https://github.com/{repo}.git'
    if (not re.fullmatch(r'https://github.com/[\w.-]+/[\w.-]+\.git', url)
            or any(part in {'.', '..'} for part in url.removeprefix('https://github.com/').removesuffix('.git').split('/'))):
        raise ValueError('Only the pinned public GitHub repository URL is accepted')
    bare = cache / f'{repo}.git'
    checkout = cache / repo / commit
    def git(args, cwd=None, timeout=600):
        result = subprocess.run(['git', *args], cwd=cwd, capture_output=True,
                                text=True, timeout=timeout)
        if result.returncode:
            raise RuntimeError(result.stderr.strip())
        return result.stdout.strip()
    if checkout.exists():
        if git(['rev-parse', 'HEAD'], checkout) != commit:
            raise ValueError(f'Unexpected checkout at {checkout}')
        if git(['status', '--porcelain'], checkout):
            raise ValueError(f'Dirty benchmark checkout at {checkout}')
        return checkout
    if not bare.exists():
        bare.parent.mkdir(parents=True, exist_ok=True)
        git(['init', '--bare', str(bare)])
        git(['remote', 'add', 'origin', url], bare)
    if git(['remote', 'get-url', 'origin'], bare) != url:
        raise ValueError('Cached repository origin differs from frozen URL')
    try:
        git(['cat-file', '-e', commit + '^{commit}'], bare)
    except RuntimeError:
        git(['fetch', '--depth=1', '--no-tags', 'origin', commit], bare)
    checkout.parent.mkdir(parents=True, exist_ok=True)
    git(['worktree', 'add', '--detach', str(checkout), commit], bare)
    if git(['rev-parse', 'HEAD'], checkout) != commit:
        raise ValueError('Checkout did not match frozen commit')
    return checkout


def terms(text):
    text = re.sub(r'([a-z])([A-Z])', r'\1 \2', text)
    return re.findall(r'[a-z0-9]+', text.lower())


class BM25:
    """Deterministic Okapi BM25 over the same per-file text used by embeddings."""
    def __init__(self, documents):
        self.documents = [(path, Counter(terms(text))) for path, text in documents]
        self.lengths = [sum(counts.values()) for _, counts in self.documents]
        self.average_length = sum(self.lengths) / max(len(self.lengths), 1) or 1.
        self.frequency = Counter()
        for _, counts in self.documents:
            self.frequency.update(counts.keys())
    def rank(self, query):
        scored = []
        n = len(self.documents)
        for (path, counts), length in zip(self.documents, self.lengths):
            score = 0.
            for term in set(terms(query)):
                frequency = counts.get(term, 0)
                df = self.frequency[term]
                idf = math.log(1 + (n - df + .5) / (df + .5))
                score += idf * frequency * 2.5 / (frequency + 1.5 * (.25 + .75 * length / self.average_length))
            scored.append((path, score))
        scored.sort(key=lambda pair: (-pair[1], str(pair[0])))
        return [path for path, _ in scored]


def merged_ranges(ranges):
    merged = []
    for start, end in sorted(ranges):
        if start < 0 or end < start:
            raise ValueError('Invalid source byte range')
        if merged and start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(end, merged[-1][1]))
        else:
            merged.append((start, end))
    return merged


def retained_span_metrics(gold_blocks, result, root):
    """Exact byte coverage of gold line intervals, validated against delivered text.

    A retained signature on a function's first line gets zero credit for its body.
    Overlapping gold blocks and duplicated class headers are unioned for byte recall.
    Generated placeholders never receive credit. Newline bytes count as source.
    """
    root = Path(root).resolve()
    delivered = defaultdict(list)
    sources = {}
    missing = []
    for chunk in result.chunks:
        path = Path(chunk['file']).resolve()
        relative = str(path.relative_to(root))
        if path not in sources:
            sources[path] = path.read_bytes()
        raw = sources[path]
        text = result.text[chunk['start_char']:chunk['end_char']].encode()
        cursor = 0
        for span in chunk['source_spans']:
            start, end = span['start_byte'], span['end_byte']
            if not 0 <= start <= end <= len(raw):
                raise ValueError('Delivered manifest span outside source')
            retained = raw[start:end]
            position = text.find(retained, cursor)
            if position < 0:
                raise ValueError('Manifest claims source absent from delivered text')
            cursor = position + len(retained)
            delivered[relative].append((start, end))
    delivered = {p: merged_ranges(ranges) for p, ranges in delivered.items()}
    gold_ranges = defaultdict(list)
    full = partial = missed = 0
    for block in gold_blocks:
        path = (root / block.file).resolve()
        if not path.is_relative_to(root):
            raise ValueError('Gold path escapes repository')
        try:
            raw = sources.get(path)
            if raw is None:
                raw = path.read_bytes(); sources[path] = raw
            parts = raw.split(b'\n')
            lines = [part + b'\n' for part in parts[:-1]] + ([parts[-1]] if parts[-1] else [])
            if not 1 <= block.start_line <= block.end_line <= len(lines):
                raise ValueError('Gold lines outside base source')
            start = sum(map(len, lines[:block.start_line - 1]))
            end = start + sum(map(len, lines[block.start_line - 1:block.end_line]))
        except (OSError, ValueError) as exc:
            missing.append({'file': block.file, 'reason': str(exc)})
            missed += 1
            continue
        gold_ranges[block.file].append((start, end))
        covered = sum(max(0, min(end, hi) - max(start, lo))
                      for lo, hi in delivered.get(block.file, []))
        if covered == end - start:
            full += 1
        elif covered:
            partial += 1
        else:
            missed += 1
    total = covered_total = 0
    for file, ranges in gold_ranges.items():
        for start, end in merged_ranges(ranges):
            total += end - start
            covered_total += sum(max(0, min(end, hi) - max(start, lo))
                                 for lo, hi in delivered.get(file, []))
    return {'gold_bytes': total, 'retained_gold_bytes': covered_total,
            'gold_byte_recall': covered_total / total if total else 0.,
            'blocks_fully_covered': full, 'blocks_partially_covered': partial,
            'blocks_missed': missed, 'invalid_gold_blocks': missing,
            'span_metrics_complete': not missing}


def summarize(rows):
    """Failures remain in the denominator, rather than improving reported means."""
    groups = defaultdict(list)
    for row in rows:
        groups[(row['strategy'], row['budget'])].append(row)
    result = []
    for (strategy, budget), group in sorted(groups.items()):
        summary = {'strategy': strategy, 'budget': budget, 'tasks': len(group),
                   'failed': sum(bool(r.get('error')) for r in group),
                   'incomplete': sum(bool(r.get('incomplete')) for r in group)}
        for metric in ('file_recall', 'file_precision', 'file_f1', 'gold_byte_recall', 'tokens'):
            summary[metric + '_mean'] = sum(r.get(metric, 0.) for r in group) / len(group)
        latencies = sorted(v for r in group for v in r.get('warm_latency_ms', []))
        if latencies:
            summary['warm_p95_ms'] = latencies[max(0, math.ceil(.95 * len(latencies)) - 1)]
        result.append(summary)
    return result


def evaluate(task, cache, budgets=(8000, 12000), repeats=3, model_identity=None):
    base = {k: task[k] for k in ('instance_id', 'repo', 'language', 'base_commit')}
    base['model'] = MODEL_NAME
    base['model_identity'] = model_identity
    rows = []
    try:
        t = time.perf_counter(); root = pinned_checkout(task, cache)
        checkout_ms = (time.perf_counter() - t) * 1000
        # The same capture/configuration/validity policy used by MCP. Close the
        # hint watcher before timing; pinned checkouts do not change in this run.
        from atlas.session import RepositoryState
        t = time.perf_counter(); state = RepositoryState(root)
        try:
            state.reconcile()
            graph, coverage = state.graph, state.coverage()
        finally:
            state.close()
        graph_ms = (time.perf_counter() - t) * 1000
        from atlas.embeddings import EmbeddingManager
        from atlas.context import ContextManager
        t = time.perf_counter(); embeddings = EmbeddingManager(model_name=MODEL_NAME, repo_graph=graph, project_root=root)
        # The captured checkout is immutable throughout this task, as in the
        # session's accepted generation. Warm queries can reuse its source keys.
        embeddings.generation_managed = True
        if model_identity is not None and embeddings.engine_key != model_identity:
            raise ValueError('Embedding weights/config changed during this run')
        files = [Path(p) for p, _ in graph.get_top_ranked_files(graph.get_statistics().node_count)]
        # Prepare all embeddings before warm measurement, including the query vector.
        query = task['problem_statement']
        embeddings.find_relevant_files_scored(query, files, top_n=len(files))
        embedding_prepare_ms = (time.perf_counter() - t) * 1000
        t = time.perf_counter()
        lexical = BM25([(p, embeddings._get_embedding_text(p)) for p in files])
        lexical_prepare_ms = (time.perf_counter() - t) * 1000
        eligible_hash = hashlib.sha256('\n'.join(sorted(str(p.relative_to(root)) for p in files)).encode()).hexdigest()
        gold = parse_gold_context(task['gold_context'])
        gold_files = {b.file for b in gold}
        for budget in budgets:
            strategies = list(STRATEGIES)
            random.Random(SEED + int(hashlib.sha256((task['instance_id'] + str(budget)).encode()).hexdigest(), 16)).shuffle(strategies)
            for strategy in strategies:
                manager = ContextManager(graph, embeddings, max_tokens=budget)
                # Reserve exactly the worker's coverage envelope for every arm.
                coverage_chars = len(json.dumps({"coverage": coverage})) + 16
                coverage_tokens = manager.count_tokens(json.dumps({"coverage": coverage}) + "\nCOVERAGE: ") + 16
                packing = {"max_tokens": max(0, budget-coverage_tokens), "max_chars": 60000-coverage_chars}
                durations, assembly_durations = [], []
                result = None
                from atlas.mcp_server import _render
                for _ in range(repeats + 1):
                    start = time.perf_counter()
                    if strategy == 'bm25':
                        candidates = [(p, 'lexical') for p in lexical.rank(query)]
                        result = manager.pack_ranked_context_result(query, candidates, generation=task['base_commit'], **packing)
                    elif strategy == 'flat_embedding':
                        scored = embeddings.find_relevant_files_scored(query, files, top_n=len(files))
                        result = manager.pack_ranked_context_result(query,
                            [(p, 'embedding') for p, _ in scored], generation=task['base_commit'], **packing)
                    else:
                        result = manager.assemble_context_result(query, [], generation=task['base_commit'],
                            expand=strategy == 'atlas', **packing)
                    assembly_durations.append((time.perf_counter() - start) * 1000)
                    payload = result.to_dict(); payload['coverage'] = coverage
                    delivered_json = _render(payload, 'json')
                    delivered_text = _render(payload, 'text')
                    durations.append((time.perf_counter() - start) * 1000)
                delivered_tokens = manager.count_tokens(delivered_text)
                json_tokens = manager.count_tokens(delivered_json)
                if max(delivered_tokens, json_tokens) > budget:
                    raise ValueError('Rendered context exceeds the requested token budget')
                retrieved = {str(Path(c['file']).relative_to(root)) for c in result.chunks}
                recall, precision, f1 = compute_file_metrics(gold_files, retrieved)
                spans = retained_span_metrics(gold, result, root)
                rows.append({**base, 'strategy': strategy, 'budget': budget,
                    'file_recall': recall, 'file_precision': precision, 'file_f1': f1,
                    'gold_files': sorted(gold_files), 'retrieved_files': sorted(retrieved),
                    **spans, 'tokens': delivered_tokens,
                    'characters': len(delivered_text), 'json_characters': len(delivered_json),
                    'json_tokens': json_tokens, 'coverage': coverage,
                    'omissions': result.counts['omission_reasons'],
                    'incomplete': bool(coverage['invalid_count']) or bool(spans['invalid_gold_blocks']) or bool(result.counts['omission_reasons'].get('source_unavailable')),
                    'cold_assembly_ms': assembly_durations[0], 'warm_assembly_ms': assembly_durations[1:],
                    'cold_delivery_ms': durations[0], 'warm_latency_ms': durations[1:],
                    'latency_scope': 'ranking, packing, and both MCP response formats',
                    'checkout_ms': checkout_ms, 'graph_ms': graph_ms,
                    'embedding_prepare_ms': embedding_prepare_ms, 'lexical_prepare_ms': lexical_prepare_ms,
                    'eligible_files': len(files), 'eligible_files_sha256': eligible_hash,
                    'embedding_engine_key': embeddings.engine_key, 'error': None})
    except Exception as exc:
        completed = {(r['strategy'], r['budget']) for r in rows}
        for budget in budgets:
            for strategy in STRATEGIES:
                if (strategy, budget) not in completed:
                    rows.append({**base, 'strategy': strategy, 'budget': budget,
                                 'error': f'{type(exc).__name__}: {exc}', 'incomplete': True})
    return rows


def implementation_fingerprint():
    """Fingerprint source, queries, build locks, and the actual loaded extension.

    Labels are portable across checkout locations; hashes are streamed so the
    compiled library does not create a large transient allocation.
    """
    import atlas
    package_root = Path(atlas.__file__).resolve().parent
    repository_root = package_root.parent.parent
    candidates = {}
    for path in package_root.rglob('*.py'):
        candidates['atlas/' + str(path.relative_to(package_root))] = path
    for name in ('atlas.context', 'atlas.embeddings'):
        module = importlib.import_module(name)
        candidates[name.replace('.', '/') + '.py'] = Path(module.__file__)
    extension = importlib.import_module('atlas.semantic_engine')
    candidates['atlas/semantic_engine.loaded-extension'] = Path(extension.__file__)
    benchmark_root = Path(__file__).resolve().parent
    for path in benchmark_root.rglob('*.py'):
        candidates['benchmarks/' + str(path.relative_to(benchmark_root))] = path
    rust_root = repository_root / 'rust_core'
    if rust_root.exists():
        for subtree, suffix in (('src', '*.rs'), ('queries', '*.scm')):
            for path in (rust_root / subtree).rglob(suffix):
                candidates[str(path.relative_to(repository_root))] = path
    for relative in ('pyproject.toml', 'uv.lock', 'rust_core/Cargo.toml', 'rust_core/Cargo.lock', 'Cargo.lock'):
        path = repository_root / relative
        if path.is_file():
            candidates[relative] = path
    files = {}
    for label, path in sorted(candidates.items()):
        digest = hashlib.sha256()
        with path.open('rb') as stream:
            for block in iter(lambda: stream.read(1024 * 1024), b''):
                digest.update(block)
        files[label] = digest.hexdigest()
    dependencies = {'python': platform.python_version()}
    for name in ('fastembed', 'numpy', 'tiktoken', 'tokenizers', 'pyarrow'):
        try:
            dependencies[name] = importlib.metadata.version(name)
        except importlib.metadata.PackageNotFoundError:
            dependencies[name] = 'unavailable'
    identity = {'files': files, 'dependencies': dependencies}
    identity['sha256'] = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
    return identity


def run_configuration(manifest, repeats, model_identity):
    if tuple(manifest['strategies']) != STRATEGIES:
        raise ValueError('Frozen strategies differ from this runner')
    if manifest['max_chars'] != 60000:
        raise ValueError('Frozen transport ceiling differs from this runner')
    return {'schema_version': RUNNER_SCHEMA_VERSION,
            'dataset_sha256': manifest['dataset_sha256'],
            'strategies': list(STRATEGIES), 'budgets': manifest['budgets'],
            'max_chars': manifest['max_chars'], 'repeats': repeats,
            'model': MODEL_NAME, 'model_identity': model_identity,
            'implementation_fingerprint': implementation_fingerprint()}


def validate_resume(output, manifest, manifest_sha256, configuration):
    """Validate every saved identity before retaining any measurements."""
    if output.get('manifest_sha256') != manifest_sha256 or output.get('manifest') != manifest:
        raise ValueError('Resume manifest differs from frozen manifest')
    if output.get('run_configuration') != configuration:
        raise ValueError('Resume dataset/strategies/budgets/repeats/model/implementation configuration differs')
    pinned = {task['instance_id']: task for task in manifest['tasks']}
    seen = set()
    for row in output.get('measurements', []):
        task = pinned.get(row.get('instance_id'))
        if task is None or any(row.get(key) != task[key] for key in ('repo', 'base_commit', 'language')):
            raise ValueError('Resume row repository/task identity differs')
        key = (row['instance_id'], row.get('strategy'), row.get('budget'))
        if key in seen:
            raise ValueError('Duplicate task/strategy/budget row in resume output')
        seen.add(key)
        if row.get('strategy') not in STRATEGIES or row.get('budget') not in configuration['budgets']:
            raise ValueError('Unexpected strategy/budget row in resume output')
        if row.get('model') != MODEL_NAME or row.get('model_identity') != configuration['model_identity']:
            raise ValueError('Resume row model identity differs')
        if row.get('embedding_engine_key') not in (None, configuration['model_identity']):
            raise ValueError('Resume row actual embedding engine identity differs')


def completed_tasks(rows, budgets):
    """Only a complete successful task can be skipped; partial/failing tasks rerun."""
    expected = {(strategy, budget) for strategy in STRATEGIES for budget in budgets}
    grouped = defaultdict(list)
    for row in rows:
        grouped[row['instance_id']].append(row)
    return {task for task, records in grouped.items()
            if {(r['strategy'], r['budget']) for r in records} == expected
            and len(records) == len(expected) and all(r.get('error') is None for r in records)}


def checkpoint(output, destination):
    """Replace a checkpoint atomically, including completed failure records."""
    output['summary'] = summarize(output['measurements'])
    expected = len(output['manifest']['tasks']) * len(STRATEGIES) * len(output['manifest']['budgets'])
    output['partial'] = len(output['measurements']) != expected
    output['updated_at'] = datetime.now(timezone.utc).isoformat()
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_suffix(destination.suffix + '.tmp')
    try:
        with temporary.open('w') as stream:
            json.dump(output, stream, indent=2)
            stream.write('\n'); stream.flush(); os.fsync(stream.fileno())
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    freeze = sub.add_parser('freeze')
    freeze.add_argument('--dataset', type=Path, required=True)
    freeze.add_argument('--output', type=Path, default=DEFAULT_MANIFEST)
    freeze.add_argument('--prior-results', type=Path,
                        default=Path(__file__).parent / 'results/contextbench_10.json')
    run = sub.add_parser('run')
    run.add_argument('--dataset', type=Path, required=True)
    run.add_argument('--manifest', type=Path, default=DEFAULT_MANIFEST)
    run.add_argument('--repo-cache', type=Path, default=Path('/tmp/atlas-contextbench'))
    run.add_argument('--output', type=Path, required=True)
    run.add_argument('--repeats', type=int, default=3)
    run.add_argument('--resume', action='store_true', help='Validate and resume an existing output; partial/failed tasks rerun')
    run.add_argument('--limit', type=int, help='Smoke only; results are explicitly marked partial')
    args = parser.parse_args()
    if args.command == 'freeze':
        if args.output.exists():
            raise SystemExit('Frozen manifest already exists; use a new versioned path')
        prior = json.loads(args.prior_results.read_text())
        excluded = {r['instance_id'] for r in prior['measurements']}
        manifest = frozen_manifest(dataset_rows(args.dataset), hashlib.sha256(args.dataset.read_bytes()).hexdigest(), excluded)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(manifest, indent=2) + '\n')
        print(f'Frozen {len(manifest["tasks"])} tasks: {args.output}')
        return
    if args.repeats < 1 or (args.limit is not None and args.limit < 1):
        raise SystemExit('repeats and limit must be positive')
    manifest = json.loads(args.manifest.read_text())
    tasks = load_frozen_tasks(args.dataset, manifest)
    manifest_sha256 = hashlib.sha256(args.manifest.read_bytes()).hexdigest()
    # One startup identity check protects resume against locally changed weights,
    # tokenizer/config, or model implementation. evaluate checks it again per task.
    from atlas.embeddings import EmbeddingManager
    model = EmbeddingManager(model_name=MODEL_NAME)
    configuration = run_configuration(manifest, args.repeats, model.engine_key)
    del model
    if args.resume:
        if not args.output.exists():
            raise SystemExit('--resume requires an existing output file')
        output = json.loads(args.output.read_text())
        validate_resume(output, manifest, manifest_sha256, configuration)
    else:
        if args.output.exists():
            raise SystemExit('Output already exists; use --resume or a new output path')
        output = {'manifest': manifest, 'manifest_sha256': manifest_sha256,
                  'run_configuration': configuration,
                  'started_at': datetime.now(timezone.utc).isoformat(),
                  'platform': platform.platform(), 'processor': platform.processor(),
                  'python': platform.python_version(),
                  'method': 'MCP session source/configuration eligibility and coverage allowance; shared map/full/skeleton packer; actual MCP rendering; 60000-character text/JSON ceiling. Warm timing excludes process IPC and manifest reconciliation, which scale benchmarks measure separately.',
                  'measurements': []}
    rows = output['measurements']
    finished = completed_tasks(rows, manifest['budgets'])
    output['status'] = 'running'
    output.pop('last_error', None)
    checkpoint(output, args.output)
    current_task = None
    try:
        for i, task in enumerate(tasks[:args.limit], 1):
            if task['instance_id'] in finished:
                print(f'{i}/{len(tasks)} {task["instance_id"]}: already complete', flush=True)
                continue
            current_task = task['instance_id']
            print(f'{i}/{len(tasks)} {current_task}', flush=True)
            results = evaluate(task, args.repo_cache, manifest['budgets'], args.repeats,
                               model_identity=configuration['model_identity'])
            # Replace all rows only after the entire task attempt completes.
            rows[:] = [row for row in rows if row['instance_id'] != current_task]
            rows.extend(results)
            checkpoint(output, args.output)
    except BaseException as exc:
        output['status'] = 'interrupted' if isinstance(exc, KeyboardInterrupt) else 'failed'
        output['last_error'] = {'task': current_task, 'type': type(exc).__name__, 'message': str(exc)}
        checkpoint(output, args.output)
        raise
    output['status'] = 'finished_with_failures' if any(row.get('error') for row in rows) else 'finished'
    checkpoint(output, args.output)
    print(json.dumps(output['summary'], indent=2))


if __name__ == '__main__':
    main()
