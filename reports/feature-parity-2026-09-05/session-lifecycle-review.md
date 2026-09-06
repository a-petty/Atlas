# Independent session/MCP/CLI lifecycle review

2026-09-05. Production changes owned by root; this reviewer staged tests only in `work/session-review-overlay/python_shell/tests/test_session_lifecycle.py`.

## Verdict and evidence

The reviewed lifecycle paths pass the bounded acceptance suite: **14 tests passed in9.51 seconds** against the fresh actual Rust extension, spawned worker processes and a real stdio MCP client. Log: `work/session-lifecycle-tests.log`.

Verified paths:

- Edit, add, delete and rename reconciliation with the native watcher disabled, proving correctness without hints.
- Stable generation on unchanged requests and new generations after accepted edits.
- Exact CRLF/UTF-8 source preservation and graph/skeleton answers remaining on captured buffers until next reconciliation.
- Syntax error removes stale declarations while preserving valid files; repair restores coverage.
- Source-root configuration, .atlasignore changes and malformed-configuration repair.
- Complete pagination, query-bound cursors, stale-generation rejection and actual MCP cursor roundtrip.
- Concurrent requests serialize into one consistent generation.
- Hard timeout kills and joins the worker, proves PID disappearance, and starts a clean process on retry.
- Active cancellation joins its worker; queued cancellation preserves the unrelated active request; cancellation before thread dispatch does not retain a global request registry.
- CLI session facade and AtlasAgent initialize/query/chat paths; syntax-protected writes remain effective and valid writes enter the next generation.

## Confirmed defects closed during review

1. `Path.read_text` normalized CRLF and shifted original byte attribution. Root changed capture to preserve raw decoded bytes; exact-byte test passes.
2. Queued cancellation could terminate a different active request. Root assigned request ownership and isolated per-call cancellation state. Active and queued cancellation tests pass.
3. `TimeoutError` was caught by the broad `OSError` handler, discarding timeout diagnosis. Root added the distinct timeout path; actual timeout test passes with the intended error and process cleanup.
4. JSON-array-string cursors were pre-parsed by FastMCP for Optional[str], breaking all real-client continuation calls at validation. Root made cursors opaque versioned base64url strings; actual stdio continuation and stale rejection pass.
5. Cancellation before thread dispatch leaked UUIDs in a global cancelled-request registry because the request cleanup never ran. Root replaced registry entries with per-call threading.Event state; saturated-thread-pool cancellation regression passes.

## Limits

This is a bounded lifecycle review, not a claim that all scale, retrieval-quality or timing acceptance criteria pass. The MCP smoke covers status/ranked/skeleton tools without embedding-model preparation; semantic preparation and context token/coverage budgets are owned by root/context worker's separate tests. The CLI test uses the existing local StubClient and the intentional architecture-only context bypass, avoiding provider dependencies and paid inference. No production files were edited by this reviewer during this review.
