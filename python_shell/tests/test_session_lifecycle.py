"""Independent regressions for source generations and worker lifecycle boundaries."""
import asyncio
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import subprocess
import sys
import time

import anyio
import pytest

from atlas.session import RepositorySession, RepositoryState, SessionContext, SessionGraph, _worker


def _write(root, name, text):
    path = root / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    return path


@pytest.fixture
def state(tmp_path):
    value = RepositoryState(tmp_path)
    try:
        yield value
    finally:
        value.close()


def _files(page):
    return {Path(item['file']).name for item in page['items']}


def test_missed_watcher_events_do_not_prevent_edit_add_delete_rename(state):
    root = state.root
    _write(root, 'a.py', 'from b import beta\ndef alpha():\n    return beta()\n')
    _write(root, 'b.py', 'def beta():\n    return 1\n')
    # No hints: every change must be found by the independent manifest scan.
    state.close()
    first = state.execute('status', {})
    assert first['coverage']['indexed'] == 2
    assert state.execute('status', {})['generation'] == first['generation']
    _write(root, 'a.py', 'from c import gamma\ndef alpha():\n    return gamma()\n')
    _write(root, 'c.py', 'def gamma():\n    return 2\n')
    second = state.execute('dependencies', {'file_path': 'a.py'})
    assert second['generation'] != first['generation']
    assert _files(second) == {'c.py'}
    (root / 'c.py').rename(root / 'renamed.py')
    third = state.execute('dependencies', {'file_path': 'a.py'})
    assert third['generation'] != second['generation'] and not third['items']
    _write(root, 'a.py', 'from renamed import gamma\ndef alpha():\n    return gamma()\n')
    assert _files(state.execute('dependencies', {'file_path': 'a.py'})) == {'renamed.py'}
    (root / 'renamed.py').unlink()
    last = state.execute('dependencies', {'file_path': 'a.py'})
    assert not last['items'] and last['coverage']['indexed'] == 2


def test_accepted_buffers_preserve_exact_crlf_bytes(state):
    original = b'def current():\r\n    return "caf\xc3\xa9"\r\n'
    path = state.root / 'windows.py'
    path.write_bytes(original)
    state.execute('status', {})
    assert state.graph.get_source(str(path)).encode('utf-8') == original


def test_graph_answers_remain_on_captured_source_until_next_reconcile(state):
    path = _write(state.root, 'api.py', 'def original():\n    return 1\n')
    generation = state.execute('status', {})['generation']
    path.write_text('def replacement():\n    return 2\n')
    assert 'original' in state.graph.get_source(str(path))
    assert 'original' in state.graph.get_skeleton(str(path))
    assert state.generation == generation
    newer = state.execute('skeleton', {'file_path': 'api.py'})
    assert newer['generation'] != generation
    assert 'replacement' in ''.join(c['text'] for c in newer['items'])
    assert 'original' not in ''.join(c['text'] for c in newer['items'])


def test_syntax_error_removes_stale_declarations_and_recovers_locally(state):
    root = state.root
    _write(root, 'valid.py', 'def healthy():\n    return 1\n')
    broken = _write(root, 'editing.py', 'def old_api():\n    return 1\n')
    first = state.execute('status', {})
    broken.write_text('def unfinished(\n')
    partial = state.execute('status', {})
    assert partial['generation'] != first['generation']
    assert partial['coverage']['invalid_count'] == 1 and partial['coverage']['indexed'] == 1
    assert 'editing.py' in next(iter(partial['coverage']['invalid_files']))
    assert _files(state.execute('ranked', {})) == {'valid.py'}
    broken.write_text('def repaired():\n    return 2\n')
    final = state.execute('status', {})
    assert final['coverage']['invalid_count'] == 0 and final['coverage']['indexed'] == 2
    skeleton = state.execute('skeleton', {'file_path': 'editing.py'})
    text = ''.join(item['text'] for item in skeleton['items'])
    assert 'repaired' in text and 'old_api' not in text


def test_configuration_and_atlasignore_changes_advance_generation(state):
    root = state.root
    _write(root, 'a.py', 'from pkg import api\ndef run():\n    return api()\n')
    _write(root, 'lib/pkg.py', 'def api():\n    return 1\n')
    first = state.execute('status', {})
    _write(root, '.atlas.toml', '[project]\nsource_roots = ["lib"]\nlanguages = ["python"]\n')
    configured = state.execute('dependencies', {'file_path': 'a.py'})
    assert configured['generation'] != first['generation']
    assert _files(configured) == {'pkg.py'}
    _write(root, '.atlasignore', 'lib/\n')
    ignored = state.execute('status', {})
    assert ignored['generation'] != configured['generation']
    assert ignored['coverage']['indexed'] == 1
    (root / '.atlasignore').write_text('')
    assert state.execute('status', {})['coverage']['indexed'] == 2
    (root / '.atlas.toml').write_text('[project\n')
    with pytest.raises(Exception): state.execute('status', {})
    (root / '.atlas.toml').write_text('[project]\nsource_roots = ["lib"]\n')
    assert state.execute('status', {})['coverage']['indexed'] == 2


def test_pagination_is_complete_query_bound_and_generation_bound(state):
    for i in range(7): _write(state.root, f'file{i}.py', f'def function{i}():\n    return {i}\n')
    first = state.execute('ranked', {'limit': 2})
    names = _files(first)
    page = first
    while page['next_cursor']:
        page = state.execute('ranked', {'limit': 2, 'cursor': page['next_cursor']})
        assert not names.intersection(_files(page))
        names.update(_files(page))
    assert len(names) == first['total'] == 7
    with pytest.raises(ValueError, match='another query'):
        state.execute('map', {'limit': 2, 'cursor': first['next_cursor']})
    _write(state.root, 'file0.py', 'def changed():\n    return 88\n')
    with pytest.raises(ValueError, match='Stale cursor'):
        state.execute('ranked', {'limit': 2, 'cursor': first['next_cursor']})


def _stuck_worker(connection, root):
    Path(root, 'worker.pid').write_text(str(os.getpid()))
    connection.recv()
    connection.send({'progress': {'phase': 'stuck'}})
    while True: time.sleep(.1)


def _controlled_worker(connection, root):
    root = Path(root)
    try:
        while True:
            message = connection.recv()
            if message is None: return
            operation = message['operation']
            with (root / 'operations.log').open('a') as log:
                log.write(operation + '\n')
            connection.send({'progress': {'phase': operation}})
            if operation == 'hold':
                (root / 'holding').write_text(str(os.getpid()))
                while not (root / 'release').exists(): time.sleep(.01)
            connection.send({'result': {'operation': operation, 'pid': os.getpid()}})
    except (EOFError, BrokenPipeError):
        pass


def _alive(pid):
    try: os.kill(pid, 0)
    except ProcessLookupError: return False
    return True


def test_hard_timeout_joins_worker_and_retry_starts_clean_process(tmp_path):
    _write(tmp_path, 'api.py', 'def api():\n    return 1\n')
    session = RepositorySession(tmp_path, timeout=1.0, worker_target=_stuck_worker)
    try:
        with pytest.raises(TimeoutError, match='terminated and joined'):
            session.request('status')
        pid = int((tmp_path / 'worker.pid').read_text())
        assert not _alive(pid)
        assert session._process is None and not session.busy
        session._worker_target = _worker
        session.timeout = 10
        result = session.request('status')
        assert result['coverage']['indexed'] == 1
        assert session._process.pid != pid
    finally:
        session.close()


def test_concurrent_requests_serialize_to_one_consistent_generation(tmp_path):
    _write(tmp_path, 'api.py', 'def api():\n    return 1\n')
    session = RepositorySession(tmp_path, timeout=10)
    try:
        with ThreadPoolExecutor(max_workers=4) as pool:
            responses = list(pool.map(lambda _: session.request('status'), range(12)))
        assert len({item['generation'] for item in responses}) == 1
        _write(tmp_path, 'api.py', 'def changed():\n    return 2\n')
        with ThreadPoolExecutor(max_workers=4) as pool:
            after = list(pool.map(lambda _: session.request('status'), range(12)))
        assert len({item['generation'] for item in after}) == 1
        assert after[0]['generation'] != responses[0]['generation']
    finally:
        session.close()


async def _wait_path(path, timeout=5):
    deadline = time.monotonic() + timeout
    while not path.exists():
        if time.monotonic() > deadline: raise AssertionError(f'Timed out waiting for {path.name}')
        await anyio.sleep(.01)


def test_active_async_cancellation_joins_worker_before_restart(tmp_path):
    async def scenario():
        session = RepositorySession(tmp_path, timeout=15, worker_target=_stuck_worker)
        try:
            with anyio.CancelScope() as scope:
                async with anyio.create_task_group() as group:
                    group.start_soon(session.arequest, 'hold')
                    await _wait_path(tmp_path / 'worker.pid')
                    scope.cancel()
            pid = int((tmp_path / 'worker.pid').read_text())
            deadline = time.monotonic() + 3
            while session.busy and time.monotonic() < deadline: await anyio.sleep(.01)
            assert not session.busy and not _alive(pid)
            session._worker_target = _worker
            session.timeout = 10
            result = await session.arequest('status')
            assert result['coverage']['indexed'] == 0
        finally:
            session.close()
    anyio.run(scenario)


def test_cancelling_queued_request_does_not_kill_unrelated_active_request(tmp_path):
    async def scenario():
        session = RepositorySession(tmp_path, timeout=10, worker_target=_controlled_worker)
        results = {}
        queued_scope = []
        async def active():
            try: results['active'] = await session.arequest('hold')
            except Exception as exc: results['active_error'] = str(exc)
        async def queued():
            with anyio.CancelScope() as scope:
                queued_scope.append(scope)
                await session.arequest('cancelled_request')
        try:
            async with anyio.create_task_group() as group:
                group.start_soon(active)
                await _wait_path(tmp_path / 'holding')
                group.start_soon(queued)
                while not queued_scope: await anyio.sleep(.01)
                await anyio.sleep(.05)
                queued_scope[0].cancel()
                await anyio.sleep(.05)
                (tmp_path / 'release').write_text('go')
            assert 'active_error' not in results, results
            assert results['active']['operation'] == 'hold'
            await anyio.sleep(.1)
            assert 'cancelled_request' not in (tmp_path / 'operations.log').read_text()
        finally:
            session.close()
    anyio.run(scenario)


def test_cli_session_facades_refresh_the_same_sources(tmp_path):
    path = _write(tmp_path, 'api.py', 'def original():\n    return 1\n')
    session = RepositorySession(tmp_path, timeout=10)
    try:
        graph = SessionGraph(session)
        assert graph.get_statistics().node_count == 1
        assert 'original' in graph.get_source(str(path))
        path.write_text('def current():\n    return 2\n')
        assert 'current' in graph.get_skeleton(str(path))
    finally:
        session.close()


def test_actual_stdio_mcp_reconciles_edit_and_rejects_stale_cursor(tmp_path):
    from mcp import ClientSession, StdioServerParameters
    from mcp.client.stdio import stdio_client
    _write(tmp_path, 'api.py', 'def original():\n    return 1\n')
    _write(tmp_path, 'second.py', 'def second():\n    return 2\n')
    async def scenario():
        env = {**os.environ, 'PYTHONDONTWRITEBYTECODE': '1', 'HF_HUB_OFFLINE': '1'}
        params = StdioServerParameters(command=sys.executable,
            args=['-m', 'atlas.mcp_server', '--project-root', str(tmp_path)], env=env)
        async with stdio_client(params) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                async def call(name, **kwargs):
                    value = await session.call_tool(name, {**kwargs, 'response_format': 'json'})
                    assert not value.isError, value
                    return '\n'.join(item.text for item in value.content if hasattr(item, 'text'))
                first = json.loads(await call('atlas_status'))
                page = json.loads(await call('get_top_ranked_files', limit=1))
                assert page['next_cursor']
                continuation = json.loads(await call('get_top_ranked_files', limit=1, cursor=page['next_cursor']))
                assert continuation['generation'] == page['generation']
                assert continuation['items'][0]['file'] != page['items'][0]['file']
                _write(tmp_path, 'api.py', 'def replacement():\n    return 3\n')
                skeleton = json.loads(await call('get_file_skeleton', file_path='api.py'))
                assert skeleton['generation'] != first['generation']
                assert 'replacement' in ''.join(c['text'] for c in skeleton['items'])
                assert 'original' not in ''.join(c['text'] for c in skeleton['items'])
                stale = await call('get_top_ranked_files', limit=1, cursor=page['next_cursor'])
                assert 'Stale cursor' in stale
    asyncio.run(scenario())


def test_cancelled_before_thread_dispatch_does_not_leak_request_ids(tmp_path):
    """Cancellation while AnyIO's pool is full never starts request's finally."""
    import threading
    async def scenario():
        session = RepositorySession(tmp_path, timeout=10)
        limiter = anyio.to_thread.current_default_thread_limiter()
        previous = limiter.total_tokens
        limiter.total_tokens = 1
        entered, release = threading.Event(), threading.Event()
        def occupy_pool():
            entered.set()
            release.wait(timeout=3)
        try:
            async with anyio.create_task_group() as group:
                group.start_soon(anyio.to_thread.run_sync, occupy_pool)
                while not entered.is_set(): await anyio.sleep(.001)
                for _ in range(20):
                    with anyio.move_on_after(.005):
                        await session.arequest('status')
                release.set()
            assert not getattr(session, "_cancelled_requests", set())
        finally:
            release.set()
            limiter.total_tokens = previous
            session.close()
    anyio.run(scenario)


def test_cli_agent_initialize_query_chat_and_syntax_protected_write(tmp_path):
    from atlas.agent import AtlasAgent
    path = _write(tmp_path, 'api.py', 'def original():\n    return 1\n')
    agent = AtlasAgent(tmp_path, provider='stub')
    try:
        agent.initialize()
        # Existing CLI architecture-only requests deliberately bypass semantic
        # retrieval. The local stub avoids any provider/model dependencies.
        agent.query('Describe the entire codebase')
        agent.chat('Describe the entire codebase')
        assert agent.llm.call_count == 2
        assert len(agent.conversation_history) == 2
        rejected = agent.tools.write_file('api.py', 'def invalid(\n')
        assert not rejected['success']
        assert 'original' in path.read_text()
        accepted = agent.tools.write_file('api.py', 'def updated():\n    return 3\n')
        assert accepted['success']
        assert 'updated' in agent.repo_graph.get_skeleton(str(path))
    finally:
        agent.stop()


def test_symlink_retarget_starts_new_generation_and_rejects_old_cursor(tmp_path):
    from atlas.session import RepositoryState
    a, b, alias = (tmp_path / name for name in ("a.py", "b.py", "link.py"))
    a.write_text("def alpha_0(): pass\ndef alpha_1(): pass\n")
    b.write_text("def bravo_0(): pass\ndef bravo_1(): pass\n")
    alias.symlink_to(a)
    state = RepositoryState(tmp_path)
    state.close()  # The generation guarantee cannot depend on native hints.
    try:
        first = state.execute("skeleton", {"file_path": "link.py", "limit": 1})
        alias.unlink(); alias.symlink_to(b)
        # Before reconciliation even aliases stay bound to accepted identities.
        assert state._path("link.py") == str(a)
        with pytest.raises(ValueError, match="Stale cursor"):
            state.execute("skeleton", {"file_path": "link.py", "limit": 1, "cursor": first["next_cursor"]})
        second = state.execute("skeleton", {"file_path": "link.py", "limit": 1})
        assert first["generation"] != second["generation"]
        assert "bravo_0" in second["items"][0]["text"]
    finally:
        state.close()


def test_configured_cold_deadline_stays_bounded_and_explicit_timeout_wins(tmp_path, monkeypatch):
    from atlas.session import RepositorySession
    monkeypatch.setenv("ATLAS_REQUEST_TIMEOUT", "900")
    session = RepositorySession(tmp_path)
    assert session.timeout == 900
    session.close()
    session = RepositorySession(tmp_path, timeout=0.1)
    assert session.timeout == 0.1
    session.close()
    for invalid in ("0", "-1", "nan", "inf", "nonsense"):
        monkeypatch.setenv("ATLAS_REQUEST_TIMEOUT", invalid)
        with pytest.raises(ValueError):
            RepositorySession(tmp_path)
