# Scale performance validation

Both the 10,000-source synthetic fixture and the pinned Django repository pass every measured warm latency gate on the final tested source. Ten samples were retained for each operation; no outliers were removed. Cold preparation remains a material operating cost, and its measured timeout/restart history is preserved separately.

| Operation | Target p95 | Synthetic 10,000 files | Pinned Django |
|---|---:|---:|---:|
| dependencies | 0.500s | 0.248s PASS | 0.301s PASS |
| map | 0.500s | 0.314s PASS | 0.329s PASS |
| search | 2.000s | 0.355s PASS | 0.368s PASS |
| context | 2.000s | 1.095s PASS | 0.678s PASS |
| edit next answer | 2.000s | 0.885s PASS | 1.723s PASS |

## What was measured

The harnesses call the normal `RepositorySession` API, using its actual spawned worker, release Rust extension, local BGE embeddings, automatic source reconciliation and bounded context delivery. Every edit changes one function body, measures the filesystem write through the very next context response, checks the accepted bytes after the timed response, and restores the original source. Restored bytes were independently verified against the deterministic manifest and pinned Django checkout after the runs.

Measurements began after the full verifier and actual-model delivery smoke passed, with other agents and compilation paused for an exclusive CPU window. Nearest-rank p95 over ten samples is the maximum observed value. The reports record every sample, query, source identity, Python source hash and extension fingerprint. Both final runs use exactly the same verified production source and extension.

The synthetic fixture has exactly 10,000 files: 2,000 each Python, JavaScript, TypeScript, Rust and Go. Seed: `20260905`. Source size: 3,338,612 bytes. Manifest SHA256: `cd091ac6833e0fc977529774fe5b47a41867edffbda349a5286a52faf37ede14`. Python and JS/TS include cross-file resolver chains. Rust and Go exercise parser/skeleton coverage, consistent with the accepted resolver scope.

The public fixture is `django/django` at full commit `8c3bd0b708b488a1f6e8bd8cc6b96569904605be`, copied into an isolated editable workspace and verified byte-for-byte before measuring. It contains 2,803 eligible source files: 2,695 Python and 108 JavaScript, totaling 15,979,072 bytes. Atlas accepts 2,799 and explicitly reports four syntax-invalid template/test fixtures. This sizeable source-heavy repository is a separate test; it does not replace the 10,000-file synthetic gate.

## Cold preparation and reuse

| Stage | Synthetic | Django |
|---|---:|---:|
| Final graph construction | 3.515s | 5.972s |
| Final model initialization and prepared-cache loading | 2.166s | 2.826s |
| Historical fresh-content-cache semantic preparation | 56.601s on earlier release | 484.834s across two attempts |

The final runs use previously prepared content caches. Synthetic cache entries remain 10,000 before and after ten edits; Django entries remain 2,799. Earlier fresh-content-cache measurements retain their own release/source provenance and are not presented as cold measurements of the final source.

Django’s first fresh-cache attempt exceeded the explicit 240s benchmark timeout. Atlas terminated and joined worker PID 61510 and retained 1,234 completed cache entries. A replacement worker, PID 61832, rebuilt its graph in 6.446s, reused those entries, and finished preparation in another 244.834s using an explicit 900s benchmark timeout. The semantic attempts total 484.834s; graph construction and restart overhead are additional and recorded separately. Warm measurements were then completed against all 2,799 accepted sources.

The normal 120s request timeout is insufficient for this entirely cold Django preparation on the measured machine. The new positive, finite `ATLAS_REQUEST_TIMEOUT` setting permits a longer bounded request while preserving the 120s default; persistent cached preparation also survives retries. No model downloads or paid model calls were used.

## Memory and cache observations

| Final prepared-cache run | After graph | After model/cache preparation | After ten edits and restore |
|---|---:|---:|---:|
| Synthetic | 0.493 GB | 0.738 GB | 0.810 GB |
| Django | 1.122 GB | 1.123 GB | 1.123 GB |

These are worker peak-RSS observations, not current resident memory. Django’s final worker peak remained 1.123 GB through all ten edits. Its earlier fresh-inference run reached a higher peak of 4.175 GB; that cold inference high-water mark is retained in the evidence. Cache-entry counts stayed constant in the final runs. Ten finite edit cycles demonstrate observed stability and do not prove asymptotic memory bounds.

## Failures retained for comparison

The earlier uncontended release failed synthetic context p95 at 3.395s, public context at 5.093s, and public edit-through-next-context at 3.510s. The final source passes those same fixture/query/edit gates. A separate early locked-environment run overlapped retrieval preparation and is labeled diagnostic/contended; none of its samples were silently dropped or mixed into acceptance results.

## Build and environment

Release extension SHA256: `80a276b6d0bbfba972a408a702515916236ccbdd9c56672f0a61788a511ba3a5`.
Git commit recorded by both final runs: `d90e59d90701bc9059385e80f9c626dca77f846a`. Per-file Atlas Python hashes and benchmark hashes are embedded in the JSON reports and were checked again after measurement.

Environment: Python 3.11.14 from `work/locked-venv`, locked FastEmbed 0.7.4, local `BAAI/bge-small-en-v1.5`, macOS 15.7.2 arm64, 14 logical CPUs. The sandbox did not expose CPU model or total RAM; those fields remain unavailable rather than inferred.

## Reproduction

From the Atlas repository, with its built release extension and the stated prepared caches:

```sh
PYTHONPATH=/Users/apetty/Dev/Atlas:/Users/apetty/Dev/Atlas/python_shell HF_HUB_OFFLINE=1 \
  /Users/apetty/Documents/Codex/2026-09-05/i/work/locked-venv/bin/python benchmarks/bench_scale.py \
  --fixture /Users/apetty/Documents/Codex/2026-09-05/i/work/scale-fixture \
  --cache /Users/apetty/Documents/Codex/2026-09-05/i/work/scale-locked-cold-cache \
  --output /Users/apetty/Documents/Codex/2026-09-05/i/work/implementation/scale-reproduction.json \
  --build-profile release --samples 10 --cold-timeout 180 --warm-timeout 30

PYTHONPATH=/Users/apetty/Dev/Atlas:/Users/apetty/Dev/Atlas/python_shell HF_HUB_OFFLINE=1 \
  /Users/apetty/Documents/Codex/2026-09-05/i/work/locked-venv/bin/python benchmarks/bench_public_scale.py \
  --repository /Users/apetty/Documents/Codex/2026-09-05/i/work/public-scale-django \
  --repo-name django/django --commit 8c3bd0b708b488a1f6e8bd8cc6b96569904605be \
  --cache /Users/apetty/Documents/Codex/2026-09-05/i/work/public-scale-django-cache \
  --output /Users/apetty/Documents/Codex/2026-09-05/i/work/implementation/public-scale-reproduction.json \
  --samples 10 --cold-timeout 900
```

## Evidence files

- `scale-accepted.json` / `.log` and `public-scale-accepted.json` / `.log`: final passing measurements.
- `scale-final.json` / `.log` and `public-scale-resumed.json` / `.log`: uncontended failures before the final fixes.
- `public-scale-final.json` / `.log`: genuine fresh-cache cold timeout.
- `scale-release.json` and `scale-locked-release.json`: earlier cold provenance; the latter is explicitly contended diagnostic evidence.
- `public-scale-copy-provenance.json` and `public-scale-accepted.manifest.json`: pinned public source identity.
- `scale-validation-before-fixes.md`: preserved earlier failure report.

The six benchmark fixture/protocol tests pass against the real extension, including all production language suffixes and edit restoration behavior.
