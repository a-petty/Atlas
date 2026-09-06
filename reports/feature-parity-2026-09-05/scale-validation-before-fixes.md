# Scale validation: preserved failures before final tuning

Measured results currently fail the context latency gate on both fixtures and the edit latency gate on Django. The context worker is investigating these failures. The evidence is retained for comparison with the next measured release.

| Operation | Target p95 | Synthetic 10,000 files | Pinned Django |
|---|---:|---:|---:|
| dependencies | 0.500s | 0.244s PASS | 0.266s PASS |
| map | 0.500s | 0.285s PASS | 0.279s PASS |
| search | 2.000s | 0.333s PASS | 0.312s PASS |
| context | 2.000s | 3.395s FAIL | 5.093s FAIL |
| edit next answer | 2.000s | 1.350s PASS | 3.510s FAIL |

All ten samples are retained for every operation. Nearest-rank p95 equals the largest observation for ten samples. Measurements ran in the agreed exclusive CPU window with the release extension and clean locked Python environment. Nothing was sampled from a reduced source subset.

## Cold preparation

The synthetic run constructed its graph in 3.999s, then loaded its prepared content cache and initialized the model in 2.695s. Earlier fresh-content-cache runs remain separate evidence: 56.601s on the earlier release and 58.379s in the clean environment diagnostic, whose early stages were contended. These earlier cold measurements are not relabeled as the final release.

Django began with an empty vector cache. Its first graph took 5.663s; semantic preparation exceeded 240s and the worker was terminated and joined. The timeout left 1234 persistent entries. A replacement worker rebuilt the graph in 6.446s and completed the remaining preparation in 244.834s. The semantic attempts therefore total 484.834s, with graph/restart costs reported separately. The resumed cache grew from 1234 to 2799 entries, and the worker PID changed from 61510 to 61832.

The normal 120s request timeout is insufficient for this entirely cold public repository on this machine. A bounded longer timeout or resumable retries are required; this limitation must be retained in the release report.

## Memory observations

| Fixture | After graph | After semantic preparation | After ten edits and restore |
|---|---:|---:|---:|
| Synthetic | 0.514 GB | 0.757 GB | 0.825 GB |
| Django | 1.134 GB | 4.060 GB | 4.175 GB |

These are peak RSS values, not current resident memory. Django peak RSS largely plateaued after the fourth edit; the synthetic run plateaued by the ninth edit. Ten finite cycles cannot prove asymptotic memory bounds. The public run includes the high peak reached during fresh embedding inference, whereas the synthetic final run loads an existing vector cache.

## Provenance

Release extension SHA256: `80a276b6d0bbfba972a408a702515916236ccbdd9c56672f0a61788a511ba3a5`. Both measured reports record Git commit `a796d490238bb0b686e1ba4b517518ec68b2f50e` and per-file Python source hashes. Python 3.11.14, FastEmbed 0.7.4, local `BAAI/bge-small-en-v1.5`, macOS 15.7.2 arm64, 14 logical CPUs. CPU model and total RAM were unavailable under the execution sandbox and are not invented.

Synthetic fixture: 10,000 files, 2,000 each Python, JavaScript, TypeScript, Rust and Go; seed 20260905; 3,338,612 source bytes; manifest digest `cd091ac6833e0fc977529774fe5b47a41867edffbda349a5286a52faf37ede14`. Python and JS/TS resolve cross-file chains. Rust and Go provide parser/skeleton coverage, consistent with the agreed resolver scope.

Public fixture: `django/django` commit `8c3bd0b708b488a1f6e8bd8cc6b96569904605be`. All 2,803 eligible source files were copied byte-for-byte into an isolated editable workspace, totaling 15,979,072 bytes (2,695 Python and 108 JavaScript). The graph accepts 2,799 files and explicitly rejects four syntax-invalid template/test fixtures. The public repository does not substitute for the synthetic 10,000-file gate.

Evidence: `scale-final.json`, `scale-final.log`, `public-scale-final.json`, `public-scale-final.log`, `public-scale-resumed.json`, `public-scale-resumed.log`, and `public-scale-copy-provenance.json`. Original cold and contended reports remain intact. The next run must use new report names so these failures stay auditable.
