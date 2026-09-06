"""Summarize frozen retrieval evidence without changing production or tuning ranks."""
import argparse
import ast
from collections import Counter, defaultdict
import hashlib
import json
import math
from pathlib import Path
import random
import re
import statistics
import subprocess
import sys


def mean(rows, field):
    return sum(row.get(field, 0.) for row in rows) / len(rows) if rows else 0.


def p95(values):
    values = sorted(values)
    return values[math.ceil(.95 * len(values)) - 1] if values else None


def describe(rows):
    successful = [row for row in rows if not row.get('error')]
    return dict(rows=len(rows), quality_denominator=len(rows), response_token_denominator=len(successful),
        failures=sum(bool(row.get('error')) for row in rows),
        tasks_with_nonempty_measurable_gold=sum(bool(row.get('gold_files')) and row.get('gold_bytes', 0) > 0
            for row in successful),
        tasks_with_complete_nonempty_gold=sum(bool(row.get('gold_files')) and row.get('gold_bytes', 0) > 0
            and row.get('span_metrics_complete') is True for row in successful),
        successful_measurement_quality={'denominator': len(successful),
            **{field: mean(successful, field) if successful else None for field in
                ('file_recall', 'file_precision', 'file_f1', 'gold_byte_recall')}},
        incomplete=sum(bool(row.get('incomplete')) for row in rows),
        **{field: mean(rows, field) for field in
           ('file_recall', 'file_precision', 'file_f1', 'gold_byte_recall')},
        **{field: mean(successful, field) for field in ('tokens', 'json_tokens')},
        warm_p95_ms=p95([value for row in rows for value in row.get('warm_latency_ms', [])]))


def paired_uncertainty(differences, tasks, repetitions=10000):
    """Keep tasks from the same public repository together in resampling."""
    clusters = defaultdict(list)
    for task_id, difference in differences.items():
        task = tasks[task_id]
        clusters[task.get('repo_url', task['repo'])].append(difference)
    grouped = [(sum(values), len(values)) for _, values in sorted(clusters.items())]
    if len(grouped) < 2:
        return {'method': 'paired repository-cluster percentile bootstrap', 'repetitions': 0,
            'seed': 20260905, 'repositories': len(grouped), 'ci95': None,
            'interpretation': 'Too few independent repository clusters to estimate an uncertainty interval.'}
    rng = random.Random(20260905)
    samples = []
    for _ in range(repetitions):
        drawn = rng.choices(grouped, k=len(grouped))
        samples.append(sum(total for total, _ in drawn) / sum(count for _, count in drawn))
    samples.sort()
    return {'method': 'paired repository-cluster percentile bootstrap', 'repetitions': repetitions,
        'seed': 20260905, 'repositories': len(grouped),
        'ci95': [samples[math.floor(.025 * repetitions)], samples[math.ceil(.975 * repetitions) - 1]],
        'language_quota_preserved_in_resamples': False,
        'interpretation': 'Descriptive sensitivity interval under resampling of observed repository clusters. It is not a confirmatory test, does not preserve the original equal language quotas, and is not adjusted for multiple contrasts.'}


def response_token_comparison(by_key, task_ids, budget, baseline):
    successful = [task_id for task_id in task_ids if all(not by_key[(task_id, method, budget)].get('error')
        for method in ('atlas', baseline))]
    result = {'paired_successful_responses': len(successful),
        'excluded_failed_pairs': len(task_ids) - len(successful)}
    for field in ('tokens', 'json_tokens'):
        atlas_total = sum(by_key[(task_id, 'atlas', budget)][field] for task_id in successful)
        baseline_total = sum(by_key[(task_id, baseline, budget)][field] for task_id in successful)
        result[field] = {'atlas_mean': atlas_total / len(successful) if successful else None,
            'baseline_mean': baseline_total / len(successful) if successful else None,
            'mean_delta': (atlas_total - baseline_total) / len(successful) if successful else None,
            'aggregate_reduction_percent': 100 * (1 - atlas_total / baseline_total) if baseline_total else None}
    return result


def paired_examples(by_key, tasks, budget, baseline, metric):
    examples = []
    for task_id, task in tasks.items():
        atlas, comparator = (by_key[(task_id, method, budget)] for method in ('atlas', baseline))
        delta = atlas.get(metric, 0.) - comparator.get(metric, 0.)
        if abs(delta) <= 1e-12:
            continue
        gold = set(atlas.get('gold_files', comparator.get('gold_files', [])))
        atlas_gold = gold & set(atlas.get('retrieved_files', []))
        baseline_gold = gold & set(comparator.get('retrieved_files', []))
        fields = ('file_recall', 'file_f1', 'gold_byte_recall', 'tokens', 'json_tokens',
            'blocks_partially_covered', 'invalid_gold_blocks', 'error')
        examples.append({'instance_id': task_id, 'repo': task['repo'], 'language': task['language'],
            'delta': delta, 'atlas': {key: atlas.get(key) for key in fields},
            'baseline': {key: comparator.get(key) for key in fields},
            'atlas_only_gold_files': sorted(atlas_gold - baseline_gold),
            'baseline_only_gold_files': sorted(baseline_gold - atlas_gold),
            'gold_files': sorted(gold)})
    return {'selection': 'Illustrative extremes selected after observing this sample; not representative task estimates.',
        'best': sorted((row for row in examples if row['delta'] > 0), key=lambda row: (-row['delta'], row['instance_id']))[:3],
        'worst': sorted((row for row in examples if row['delta'] < 0), key=lambda row: (row['delta'], row['instance_id']))[:3]}


def preparation_costs(rows, tasks):
    fields = ('checkout_ms', 'graph_ms', 'embedding_prepare_ms', 'lexical_prepare_ms')
    per_task, inconsistent = {}, []
    for task_id, task in tasks.items():
        measured = [row for row in rows if row['instance_id'] == task_id and all(field in row for field in fields)]
        if not measured:
            continue
        for field in fields:
            if len({row[field] for row in measured}) > 1:
                inconsistent.append({'instance_id': task_id, 'field': field})
        values = {field.removesuffix('_ms') + '_seconds': measured[0][field] / 1000 for field in fields}
        per_task[task_id] = {'repo': task['repo'], 'language': task['language'], **values,
            'preparation_seconds': sum(measured[0][field] for field in fields[1:]) / 1000}
    distributions = {}
    for field in (*[field.removesuffix('_ms') + '_seconds' for field in fields], 'preparation_seconds'):
        values = [row[field] for row in per_task.values()]
        distributions[field] = {'tasks_measured': len(values), 'total': sum(values),
            'mean': statistics.mean(values) if values else None,
            'median': statistics.median(values) if values else None, 'p95': p95(values),
            'maximum': max(values) if values else None}
    return {'unit': 'seconds', 'unique_tasks_measured': len(per_task),
        'total_tasks': len(tasks), 'totals_cover_all_tasks': len(per_task) == len(tasks),
        'missing_task_timings': sorted(set(tasks) - set(per_task)), 'inconsistent_repeated_fields': inconsistent,
        'distributions': distributions,
        'worst_tasks_by_preparation': sorted([{'instance_id': key, **row} for key, row in per_task.items()],
            key=lambda row: -row['preparation_seconds'])[:5], 'per_task': per_task,
        'interpretation': 'Each setup stage is counted once per task, despite being repeated on up to eight result rows. These are mixed cache-hit/cache-miss preparation measurements across pinned commits, not fully cold-cache timings. Embedding preparation was shared by this experiment but is not an intrinsic BM25 requirement. The lexical stage followed embedding preparation and may benefit from already prepared source representations. Missing setup timings are not counted as zero cost.'}


def retrieval_equivalence_audit(original, corrected):
    """Compare saved retrieval evidence independently of changed gold accounting."""
    fields = ('retrieved_files', 'tokens', 'json_tokens', 'characters', 'json_characters',
        'eligible_files', 'eligible_files_sha256', 'embedding_engine_key')
    def indexed(data):
        expected = {(task['instance_id'], method, budget) for task in data['manifest']['tasks']
            for method in data['manifest']['strategies'] for budget in data['manifest']['budgets']}
        rows = {}
        for row in data['measurements']:
            key = (row['instance_id'], row['strategy'], row['budget'])
            if key in rows:
                raise ValueError(f'Duplicate result in equivalence audit: {key}')
            rows[key] = row
        if set(rows) != expected:
            raise ValueError('Equivalence audit requires both complete frozen task/method/budget grids')
        return rows
    before, after = indexed(original), indexed(corrected)
    differences, unpaired, missing_fields = [], [], []
    compared = 0
    for key, row in sorted(before.items()):
        if row.get('error'):
            continue
        current = after.get(key)
        if current is None or current.get('error'):
            unpaired.append({'instance_id': key[0], 'strategy': key[1], 'budget': key[2],
                'corrected_error': current.get('error') if current is not None else 'Missing corrected row'})
            continue
        compared += 1
        absent = {field: {'original_missing': field not in row, 'corrected_missing': field not in current}
            for field in fields if field not in row or field not in current}
        if absent:
            missing_fields.append({'instance_id': key[0], 'strategy': key[1], 'budget': key[2], 'fields': absent})
        changed = {field: {'original': row.get(field), 'corrected': current.get(field)}
            for field in fields if row.get(field) != current.get(field)}
        if changed:
            differences.append({'instance_id': key[0], 'strategy': key[1], 'budget': key[2], 'fields': changed})
    old_config, new_config = original['run_configuration'], corrected['run_configuration']
    old_fingerprint, new_fingerprint = (config['implementation_fingerprint'] for config in (old_config, new_config))
    changed_labels = sorted(label for label in set(old_fingerprint['files']) | set(new_fingerprint['files'])
        if old_fingerprint['files'].get(label) != new_fingerprint['files'].get(label))
    return {
        'original_schema_version': old_config['schema_version'],
        'corrected_schema_version': new_config['schema_version'],
        'manifests_match': original['manifest'] == corrected['manifest'] and
            original.get('manifest_sha256') == corrected.get('manifest_sha256'),
        'model_identity_matches': old_config['model_identity'] == new_config['model_identity'],
        'dependency_versions_match': old_fingerprint['dependencies'] == new_fingerprint['dependencies'],
        'changed_source_fingerprint_labels': changed_labels,
        'original_successful_rows': sum(not row.get('error') for row in before.values()),
        'original_failed_rows_excluded': sum(bool(row.get('error')) for row in before.values()),
        'paired_successful_rows_compared': compared, 'fields_compared': list(fields),
        'original_successful_rows_without_corrected_success': unpaired,
        'rows_with_missing_comparison_fields': missing_fields,
        'rows_with_differences': differences,
        'all_compared_fields_equal': compared > 0 and not differences and not missing_fields,
        'interpretation': 'This compares original successful rows with matching corrected successful rows. Original failures provide no saved retrieval evidence and are explicitly excluded. Latencies and gold metrics are not required to match. Matching file lists, response sizes, accepted-file identities, and engine keys support retrieval consistency; the saved rows do not contain full response text/chunk manifests, so this is not proof of byte-identical responses.'}


def sha256_file(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def fingerprint_audit(data, atlas_repo=None):
    saved = data['run_configuration']['implementation_fingerprint']
    computed = hashlib.sha256(json.dumps({key: saved[key] for key in ('files', 'dependencies')}, sort_keys=True).encode()).hexdigest()
    required = {'atlas/context.py', 'atlas/embeddings.py', 'atlas/session.py', 'atlas/mcp_server.py',
        'atlas/semantic_engine.loaded-extension', 'benchmarks/bench_retrieval.py'}
    result = {'recorded_sha256': saved['sha256'], 'internal_digest_valid': computed == saved['sha256'],
        'evaluator_schema_version': data['run_configuration']['schema_version'],
        'gold_path_policy': data['run_configuration'].get('gold_path_policy'),
        'required_source_labels_missing': sorted(required - set(saved['files'])),
        'source_files_fingerprinted': len(saved['files']), 'dependencies': saved['dependencies'],
        'model_identity': data['run_configuration']['model_identity'],
        'rows_with_wrong_model_identity': [row['instance_id'] for row in data['measurements']
            if row.get('model_identity') != data['run_configuration']['model_identity'] or
            row.get('embedding_engine_key', data['run_configuration']['model_identity']) != data['run_configuration']['model_identity']]}
    if atlas_repo is not None:
        sys.path[:0] = [str(atlas_repo), str(atlas_repo / 'python_shell')]
        from benchmarks.bench_retrieval import implementation_fingerprint
        from atlas.context import ContextManager
        current = implementation_fingerprint()
        result['response_token_encoding'] = ContextManager(None, None).encoder.name
        result['matches_current_implementation'] = saved == current
        result['changed_source_labels'] = sorted(key for key in set(saved['files']) | set(current['files'])
            if saved['files'].get(key) != current['files'].get(key))
        result['current_dependency_versions'] = current['dependencies']
    result['interpretation'] = 'The run records production/benchmark source, Rust queries, lockfiles, the loaded extension, model content identity, and selected dependency versions. This is code/environment provenance, not a cryptographic record of every host setting or of filesystem immutability throughout execution.'
    return result


def source_policy(atlas_repo):
    tree = ast.parse((atlas_repo / 'python_shell/atlas/session.py').read_text())
    values = {}
    for statement in tree.body:
        if isinstance(statement, ast.Assign):
            for target in statement.targets:
                if isinstance(target, ast.Name) and target.id in {'EXTENSIONS', 'IGNORED_DIRS'}:
                    values[target.id] = ast.literal_eval(statement.value)
    if set(values) != {'EXTENSIONS', 'IGNORED_DIRS'}:
        raise ValueError('Cannot recover source policy from the audited implementation')
    return values


def pinned_clean_checkout(root, commit):
    if not root.is_dir():
        return False
    head = subprocess.run(['git', '-C', str(root), 'rev-parse', 'HEAD'], capture_output=True, text=True, timeout=30)
    if head.returncode or head.stdout.strip() != commit:
        return False
    status = subprocess.run(['git', '-C', str(root), 'status', '--porcelain'], capture_output=True, text=True, timeout=30)
    return status.returncode == 0 and not status.stdout.strip()


def reconstruct_accepted_gold_paths(data, repo_cache):
    """Replay only source-manifest scans and verify exact recorded ID hashes."""
    from atlas.session import RepositoryState
    grouped = defaultdict(list)
    for row in data['measurements']:
        if not row.get('error'):
            grouped[row['instance_id']].append(row)
    result, scanned = {}, {}
    for task in data['manifest']['tasks']:
        task_id = task['instance_id']
        rows = grouped.get(task_id, [])
        if not rows:
            result[task_id] = {'verified': False, 'reason': 'No successful measurement'}
            continue
        row = rows[0]
        coverage = row['coverage']
        if (coverage.get('invalid_details_omitted') or
                len(coverage.get('invalid_files', {})) != coverage['invalid_count']):
            result[task_id] = {'verified': False, 'reason': 'Invalid-source diagnostics are truncated'}
            continue
        if len({item['eligible_files_sha256'] for item in rows}) != 1:
            result[task_id] = {'verified': False, 'reason': 'Recorded eligibility differs among methods/budgets'}
            continue
        root = (repo_cache / task['repo'] / task['base_commit']).resolve()
        if not root.is_relative_to(repo_cache.resolve()) or not root.is_dir():
            result[task_id] = {'verified': False, 'reason': 'Pinned checkout unavailable'}
            continue
        try:
            if root not in scanned:
                if not pinned_clean_checkout(root, task['base_commit']):
                    result[task_id] = {'verified': False, 'reason': 'Checkout is dirty or HEAD differs from the pinned commit'}
                    continue
                state = RepositoryState(root)
                try:
                    manifest, _ = state._scan()
                    scanned[root] = set(manifest)
                finally:
                    state.close()
            accepted = scanned[root] - set(coverage.get('invalid_files', {}))
            relative = {str(Path(path).relative_to(root)) for path in accepted}
            digest = hashlib.sha256('\n'.join(sorted(relative)).encode()).hexdigest()
            if digest != row['eligible_files_sha256'] or len(relative) != row['eligible_files']:
                result[task_id] = {'verified': False, 'reason': 'Reconstructed accepted IDs do not match the recorded hash/count'}
                continue
            result[task_id] = {'verified': True, 'eligible_files': len(relative),
                'eligible_files_sha256': digest, 'accepted_gold_files': sorted(set(row['gold_files']) & relative),
                'method': 'Unchanged production source-manifest scan minus complete recorded invalid-source diagnostics; no graph/model initialization'}
        except Exception as exc:
            result[task_id] = {'verified': False, 'reason': f'{type(exc).__name__}: {exc}'}
    return result


def gold_byte_details(data, dataset, repo_cache):
    """Re-read only pinned gold files; never rebuild graphs or execute repository code."""
    if sha256_file(dataset) != data['manifest']['dataset_sha256']:
        raise ValueError('Annotation dataset differs from the frozen dataset')
    import pyarrow.parquet as pq
    tasks = {task['instance_id']: task for task in data['manifest']['tasks']}
    columns = ['instance_id', 'repo', 'repo_url', 'language', 'base_commit', 'gold_context']
    rows = pq.read_table(dataset, columns=columns, filters=[('instance_id', 'in', list(tasks))], use_threads=False).to_pylist()
    path_policy = data['run_configuration'].get('gold_path_policy')
    if path_policy is not None:
        from benchmarks.bench_contextbench import parse_gold_context
        from benchmarks.bench_retrieval import GOLD_PATH_POLICY, normalize_gold_paths, gold_path_provenance
        if path_policy != GOLD_PATH_POLICY:
            raise ValueError('Unknown or mismatched recorded gold path policy')
    detailed, validated_roots = {}, {}
    for row in rows:
        task = tasks[row['instance_id']]
        if any(row[key] != value for key, value in task.items()):
            raise ValueError('Annotation row differs from the pinned task')
        root = (repo_cache / task['repo'] / task['base_commit']).resolve()
        if not root.is_relative_to(repo_cache.resolve()):
            detailed[row['instance_id']] = {'error': 'Checkout path escapes the requested cache'}
            continue
        if root not in validated_roots:
            validated_roots[root] = pinned_clean_checkout(root, task['base_commit'])
        if not validated_roots[root]:
            detailed[row['instance_id']] = {'error': 'Pinned checkout unavailable, dirty, or HEAD differs'}
            continue
        blocks = row['gold_context']
        if isinstance(blocks, str):
            blocks = json.loads(blocks)
        if path_policy is not None:
            normalized = normalize_gold_paths(parse_gold_context(blocks), root, task)
            provenance = gold_path_provenance(normalized, root)
            measured = [item for item in data['measurements']
                if item['instance_id'] == row['instance_id'] and not item.get('error')]
            if any(item.get('gold_path_mappings') != provenance or
                    item.get('gold_path_policy') != path_policy['id'] for item in measured):
                detailed[row['instance_id']] = {'error': 'Replayed gold path provenance differs from recorded measurements'}
                continue
            blocks = [{'file': block.file, 'original_file': block.original_file,
                'start_line': block.start_line, 'end_line': block.end_line,
                'invalid_path_reason': block.invalid_reason} for block in normalized]
        files = {}
        for block in blocks:
            relative = block['file']
            entry = files.setdefault(relative, {'ranges': [], 'invalid_blocks': []})
            try:
                if block.get('invalid_path_reason'):
                    entry['invalid_path_reason'] = block['invalid_path_reason']
                    raise ValueError(block['invalid_path_reason'])
                path = (root / relative).resolve()
                if not path.is_relative_to(root):
                    raise ValueError('Gold path escapes the pinned checkout')
                entry['canonical_file'] = str(path.relative_to(root))
                if 'raw' not in entry:
                    entry['raw'] = path.read_bytes()
                    offsets = [0, *[match.end() for match in re.finditer(b'\n', entry['raw'])]]
                    if offsets[-1] != len(entry['raw']):
                        offsets.append(len(entry['raw']))
                    entry['offsets'] = offsets
                start, end = block['start_line'], block['end_line']
                offsets = entry['offsets']
                if (type(start) is not int or type(end) is not int or
                        not 1 <= start <= end < len(offsets)):
                    raise ValueError('Gold lines outside base source')
                entry['ranges'].append((offsets[start - 1], offsets[end]))
            except (OSError, ValueError, TypeError, RuntimeError) as exc:
                entry['invalid_blocks'].append(str(exc))
        for entry in files.values():
            merged = []
            for start, end in sorted(entry.pop('ranges')):
                if merged and start <= merged[-1][1]:
                    merged[-1] = (merged[-1][0], max(end, merged[-1][1]))
                else:
                    merged.append((start, end))
            entry['known_gold_bytes'] = sum(end - start for start, end in merged)
            entry.pop('raw', None); entry.pop('offsets', None)
        detailed[row['instance_id']] = {'files': files}
    return detailed


def gold_scope_annotations(data, policy, byte_details=None, reconstructed=None):
    """Supplement the original denominator; never substitute an eligible-only score."""
    byte_details = byte_details or {}
    reconstructed = reconstructed or {}
    grouped = defaultdict(list)
    for row in data['measurements']:
        if not row.get('error'):
            grouped[row['instance_id']].append(row)
    task_reports, discrepancies = {}, []
    for task_id, rows in grouped.items():
        first = rows[0]
        gold = set(first['gold_files'])
        proven_accepted = set().union(*(set(row['retrieved_files']) for row in rows))
        reconstruction = reconstructed.get(task_id, {})
        if reconstruction.get('verified'):
            proven_accepted.update(reconstruction['accepted_gold_files'])
        invalid_paths = first['coverage'].get('invalid_files', {})
        annotations = []
        for file in sorted(gold):
            detail = byte_details.get(task_id, {}).get('files', {}).get(file)
            known_invalid = next((reason for path, reason in invalid_paths.items()
                if path == file or path.endswith('/' + file)), None)
            if file in proven_accepted:
                status, reason = 'proven_accepted', 'Delivered by some arm or present in accepted IDs verified against the recorded hash'
            elif detail is not None and detail.get('invalid_path_reason'):
                status, reason = 'known_excluded', 'Unresolved gold annotation path: ' + detail['invalid_path_reason']
            elif detail is not None and detail.get('canonical_file') not in (None, file):
                status, reason = 'eligibility_unknown', 'Gold path names a canonical-path alias; literal file metrics may differ from source identity'
            elif Path(file).suffix.lower() not in policy['EXTENSIONS']:
                status, reason = 'known_excluded', 'Unsupported source extension'
            elif any(part in policy['IGNORED_DIRS'] for part in Path(file).parts):
                status, reason = 'known_excluded', 'Excluded directory in the recorded source policy'
            elif known_invalid is not None:
                status, reason = 'known_excluded', 'Reported invalid source: ' + known_invalid
            elif reconstruction.get('verified'):
                status, reason = 'known_excluded', 'Absent from accepted source IDs exactly matching the recorded hash/count'
            else:
                status, reason = 'eligibility_unknown', 'Supported extension, but not delivered and absent from bounded invalid-file diagnostics; the eligible hash does not enumerate paths'
            annotations.append({'file': file, 'eligibility': status, 'reason': reason,
                'invalid_gold_path': bool(detail and detail.get('invalid_path_reason')),
                'known_gold_bytes': detail['known_gold_bytes'] if detail is not None else None,
                'unmeasurable_blocks': len(detail['invalid_blocks']) if detail is not None else None})
        excluded = {row['file'] for row in annotations if row['eligibility'] == 'known_excluded'}
        accepted_gold = gold & proven_accepted
        byte_complete = all(row['known_gold_bytes'] is not None for row in annotations)
        known_bytes = sum(row['known_gold_bytes'] for row in annotations) if byte_complete else None
        excluded_bytes = sum(row['known_gold_bytes'] for row in annotations if row['file'] in excluded) if byte_complete else None
        if byte_complete and known_bytes != first['gold_bytes']:
            discrepancies.append({'instance_id': task_id, 'recorded': first['gold_bytes'], 'annotation': known_bytes})
        task_reports[task_id] = {'gold_files': len(gold), 'known_excluded_files': len(excluded),
            'unknown_eligibility_files': sum(row['eligibility'] == 'eligibility_unknown' for row in annotations),
            'file_recall_upper_bound_from_known_exclusions': (len(gold) - len(excluded)) / len(gold) if gold else None,
            'known_gold_bytes': known_bytes, 'known_excluded_gold_bytes': excluded_bytes,
            'known_byte_recall_upper_bound_from_known_exclusions': 1 - excluded_bytes / known_bytes if known_bytes else None,
            'eligible_id_reconstruction': reconstruction,
            'files': annotations,
            'measurements': [{'strategy': row['strategy'], 'budget': row['budget'],
                'proven_accepted_gold_files_missed': sorted(accepted_gold - set(row['retrieved_files'])),
                'unattributed_gold_files_missed': sorted(gold - excluded - accepted_gold - set(row['retrieved_files'])),
                'partially_covered_gold_blocks': row['blocks_partially_covered'],
                'all_gold_files_returned_but_known_source_incomplete': row['file_recall'] == 1. and row['retained_gold_bytes'] < row['gold_bytes']}
                for row in rows]}
    return {'byte_annotation_discrepancies': discrepancies, 'tasks': task_reports,
        'known_excluded_gold_file_occurrences': sum(row['known_excluded_files'] for row in task_reports.values()),
        'invalid_gold_path_file_occurrences': sum(file['invalid_gold_path']
            for task in task_reports.values() for file in task['files']),
        'known_excluded_gold_bytes': sum(row['known_excluded_gold_bytes'] or 0 for row in task_reports.values()),
        'tasks_with_byte_annotations': sum(row['known_gold_bytes'] is not None for row in task_reports.values()),
        'annotation_errors': {task_id: detail['error'] for task_id, detail in byte_details.items() if 'error' in detail},
        'interpretation': 'Primary file and byte denominators are unchanged. Known exclusions establish an upper bound on recall, not an adjusted score. Acceptance is proven by delivery or by replayed source-manifest IDs exactly matching the recorded hash/count. Failed reconstruction, truncated diagnostics, and canonical-path aliases remain explicitly unknown where other evidence does not establish eligibility. Missing a proven accepted gold file is a ranking/packing selection miss. Partial retained gold blocks prove incomplete source retention. The saved fields cannot generally separate ranking from packing or attribute every missed byte to a file.'}


def analyze(data):
    manifest, rows = data['manifest'], data['measurements']
    expected = {(task['instance_id'], method, budget) for task in manifest['tasks']
        for method in manifest['strategies'] for budget in manifest['budgets']}
    by_key = {}
    for row in rows:
        key = (row['instance_id'], row['strategy'], row['budget'])
        if key in by_key:
            raise ValueError(f'Duplicate result {key}')
        if key not in expected:
            raise ValueError(f'Unexpected result {key}')
        by_key[key] = row
    if set(by_key) != expected:
        raise ValueError(f'Result is incomplete: {len(by_key)} of {len(expected)} expected rows')
    tasks = {task['instance_id']: task for task in manifest['tasks']}
    grouped, language_groups = defaultdict(list), defaultdict(list)
    for row in rows:
        grouped[(row['budget'], row['strategy'])].append(row)
        language_groups[(row['language'], row['budget'], row['strategy'])].append(row)
    budgets = []
    comparisons = []
    for budget in manifest['budgets']:
        budgets.append({'budget': budget, 'methods': {
            method: describe(grouped[(budget, method)]) for method in manifest['strategies']}})
        for baseline in ('bm25', 'flat_embedding', 'atlas_no_expansion'):
            successful_pairs = {task_id for task_id in tasks if all(
                not by_key[(task_id, method, budget)].get('error') for method in ('atlas', baseline))}
            compared = {'budget': budget, 'comparison': 'atlas minus ' + baseline,
                'primary_paired_denominator': len(tasks),
                'pairs_with_measurement_errors': sorted(set(tasks) - successful_pairs),
                'actual_response_tokens': response_token_comparison(by_key, tasks, budget, baseline)}
            for metric in ('file_f1', 'gold_byte_recall'):
                differences = {task_id: by_key[(task_id, 'atlas', budget)].get(metric, 0.) -
                    by_key[(task_id, baseline, budget)].get(metric, 0.) for task_id in tasks}
                compared[metric] = {'mean_delta': statistics.mean(differences.values()),
                    'wins': sum(value > 1e-12 for value in differences.values()),
                    'ties': sum(abs(value) <= 1e-12 for value in differences.values()),
                    'losses': sum(value < -1e-12 for value in differences.values()),
                    'uncertainty': paired_uncertainty(differences, tasks),
                    'examples': paired_examples(by_key, tasks, budget, baseline, metric),
                    'successful_pairs_companion': {'denominator': len(successful_pairs),
                        'mean_delta': statistics.mean(differences[task_id] for task_id in successful_pairs) if successful_pairs else None,
                        'wins': sum(differences[task_id] > 1e-12 for task_id in successful_pairs),
                        'ties': sum(abs(differences[task_id]) <= 1e-12 for task_id in successful_pairs),
                        'losses': sum(differences[task_id] < -1e-12 for task_id in successful_pairs)}}
                known = {task_id: value for task_id, value in differences.items()
                    if all(by_key[(task_id, method, budget)].get('span_metrics_complete') is True
                        and by_key[(task_id, method, budget)].get('gold_bytes', 0) > 0
                        and bool(by_key[(task_id, method, budget)].get('gold_files'))
                        and not by_key[(task_id, method, budget)].get('error')
                        for method in ('atlas', baseline))}
                compared[metric]['complete_gold_subset'] = {
                    'tasks': len(known), 'mean_delta': statistics.mean(known.values()) if known else None,
                    'requirement': 'Both measurements succeeded, have nonempty gold files and positive measurable gold bytes, and report complete span metrics.',
                    'uncertainty': paired_uncertainty(known, tasks) if known else None}
            comparisons.append(compared)
    violations, eligibility_differences = [], []
    for task_id in tasks:
        task_rows = [row for row in rows if row['instance_id'] == task_id and not row.get('error')]
        if len({row['eligible_files_sha256'] for row in task_rows}) > 1:
            eligibility_differences.append(task_id)
    for row in rows:
        if row.get('error'):
            continue
        if (max(row['tokens'], row['json_tokens']) > row['budget'] or
                max(row['characters'], row['json_characters']) > manifest['max_chars']):
            violations.append([row['instance_id'], row['strategy'], row['budget']])
    unique_tasks = {}
    for row in rows:
        if not row.get('error'):
            unique_tasks.setdefault(row['instance_id'], row)
    return {
        'tasks': len(tasks), 'rows': len(rows), 'languages': dict(Counter(t['language'] for t in tasks.values())),
        'all_expected_rows_present': True, 'budget_violations': violations,
        'eligibility_differences': eligibility_differences,
        'failed_rows': [{'task': row['instance_id'], 'method': row['strategy'], 'budget': row['budget'],
            'error': row['error']} for row in rows if row.get('error')],
        'tasks_with_parse_invalid_files': sum(row['coverage']['invalid_count'] > 0 for row in unique_tasks.values()),
        'tasks_with_no_successful_measurement': sorted(set(tasks) - set(unique_tasks)),
        'known_gold_bytes': sum(row['gold_bytes'] for row in unique_tasks.values()),
        'tasks_with_no_gold_files': sorted(task_id for task_id, row in unique_tasks.items()
            if not row['gold_files']),
        'tasks_with_no_measurable_gold_bytes': sorted(task_id for task_id, row in unique_tasks.items()
            if row['gold_bytes'] == 0),
        'gold_path_policy': data['run_configuration'].get('gold_path_policy'),
        'gold_annotation_path_forms': {
            'tasks_with_recorded_provenance': sum('gold_path_mappings' in row for row in unique_tasks.values()),
            'tasks_with_absolute_original_paths': sorted(task_id for task_id, row in unique_tasks.items()
                if any(isinstance(file, str) and Path(file).is_absolute()
                    for file in row.get('gold_original_files', []))),
            'tasks_with_repository_container_paths': sorted(task_id for task_id, row in unique_tasks.items()
                if any(mapping.get('container_prefix') for mapping in row.get('gold_path_mappings', []))),
            'tasks_with_only_invalid_gold_paths': sorted(task_id for task_id, row in unique_tasks.items()
                if row.get('gold_path_mappings') and all(mapping['mapping'] == 'invalid'
                    for mapping in row['gold_path_mappings'])),
        },
        'gold_path_mapping_counts': dict(Counter(mapping['mapping']
            for row in unique_tasks.values() for mapping in row.get('gold_path_mappings', []))),
        'invalid_gold_paths': {task_id: [mapping for mapping in row.get('gold_path_mappings', [])
            if mapping['mapping'] == 'invalid'] for task_id, row in unique_tasks.items()
            if any(mapping['mapping'] == 'invalid' for mapping in row.get('gold_path_mappings', []))},
        'unmeasurable_gold_blocks': sum(len(row['invalid_gold_blocks']) for row in unique_tasks.values()),
        'tasks_with_invalid_gold_blocks': {task_id: row['invalid_gold_blocks']
            for task_id, row in unique_tasks.items() if row['invalid_gold_blocks']},
        'gold_measurability_by_task': {task_id: {
            'gold_files': len(row['gold_files']),
            'gold_bytes': row['gold_bytes'], 'unmeasurable_blocks': len(row['invalid_gold_blocks']),
            'total_gold_blocks': sum(row[field] for field in
                ('blocks_fully_covered', 'blocks_partially_covered', 'blocks_missed'))}
            for task_id, row in unique_tasks.items()},
        'budgets': budgets, 'paired_comparisons': comparisons,
        'preparation': preparation_costs(rows, tasks),
        'by_language': [{'language': language, 'budget': budget, 'method': method, **describe(group)}
            for (language, budget, method), group in sorted(language_groups.items())],
        'interpretation_limits': [
            'This is a fixed 50-task retrieval sample, not coding-task completion evidence.',
            'Failed measurements remain zero in primary metric means and paired comparisons; these are conservative imputed experiment scores, not observed retrieval quality on failed tasks. Successful-measurement and successful-pair companion summaries show their explicit smaller denominators. All-arm failures count as primary ties but are separately identified.',
            'Gold-byte recall excludes invalid gold ranges; affected tasks are listed explicitly.',
            'Tasks with empty gold or zero measurable gold bytes remain in primary denominators and are listed separately. Complete-gold companion subsets require nonempty files, positive measurable bytes, and complete span metrics in both successful measurements.',
            'For evaluator schema 3, valid relative, dot-path, internal-symlink, and verified repository-container aliases map to canonical repository-relative file IDs; unresolved gold path IDs remain in the file denominator. Invalid paths and ranges have unmeasurable bytes and are explicitly reported. Tasks with zero measurable gold bytes have no observed byte-recall evidence, despite the primary zero convention.',
            'Actual response token means use successful responses; paired token differences use successful pairs with the excluded failure count shown. Fewer tokens alone do not establish useful savings when retrieval quality is lower.',
            'Response token counts use the same Atlas/tiktoken counter on delivered text and JSON. They are not measurements of total coding-session tokens, provider billing, avoided file reads, or task completion.',
            'Bootstrap intervals resample public repositories, retaining task pairs and within-repository dependence. They describe uncertainty under this sample, not coding-task success or a guaranteed population effect.',
            'Warm delivery latency includes ranking, packing, and both MCP formats; excludes model/graph preparation.',
        ],
    }


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('result', type=Path)
    parser.add_argument('--atlas-repo', type=Path)
    parser.add_argument('--dataset', type=Path)
    parser.add_argument('--repo-cache', type=Path)
    parser.add_argument('--original-result', type=Path,
        help='Complete original run for an observed retrieval-metadata comparison after evaluator correction')
    args = parser.parse_args()
    data = json.loads(args.result.read_text())
    report = analyze(data)
    report['fingerprint_audit'] = fingerprint_audit(data, args.atlas_repo)
    if args.original_result is not None:
        original = json.loads(args.original_result.read_text())
        report['original_run_retrieval_comparison'] = retrieval_equivalence_audit(
            original, data)
        report['original_run_preparation'] = preparation_costs(original['measurements'],
            {task['instance_id']: task for task in original['manifest']['tasks']})
    if args.atlas_repo is not None:
        audit = report['fingerprint_audit']
        if audit['internal_digest_valid'] and audit.get('matches_current_implementation'):
            details = gold_byte_details(data, args.dataset, args.repo_cache) if args.dataset and args.repo_cache else None
            reconstructed = reconstruct_accepted_gold_paths(data, args.repo_cache) if args.repo_cache else None
            report['gold_scope'] = gold_scope_annotations(data, source_policy(args.atlas_repo), details, reconstructed)
        else:
            report['gold_scope'] = {'verification_failed': True,
                'reason': 'Current implementation does not verify against the recorded fingerprint; eligibility classifications are unknown and no source-policy replay was performed.'}
    destination = args.result.with_suffix('.analysis.json')
    destination.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'output': str(destination), 'tasks': report['tasks'], 'rows': report['rows'],
        'failed_rows': len(report['failed_rows']), 'budgets': report['budgets'],
        'paired_comparisons': report['paired_comparisons'],
        'preparation': report['preparation']['distributions']}, indent=2))
