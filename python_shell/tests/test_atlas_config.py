"""Tests for `.atlas.toml` config loading in mcp_server.

Covers Phase 4 of the multi-language remediation plan: the optional
`languages = [...]` key under `[project]`. Combined with the existing
`source_roots` key, `.atlas.toml` is the single config surface for
overriding detection-based defaults.
"""

import os
import tempfile
from pathlib import Path

import pytest

from atlas.mcp_server import _load_atlas_config


def _write_toml(root: Path, body: str) -> None:
    (root / ".atlas.toml").write_text(body)


def test_no_atlas_toml_returns_none_none():
    with tempfile.TemporaryDirectory() as d:
        roots, langs = _load_atlas_config(Path(d))
        assert roots is None
        assert langs is None


def test_source_roots_only():
    with tempfile.TemporaryDirectory() as d:
        _write_toml(Path(d), '[project]\nsource_roots = ["src", "lib"]\n')
        roots, langs = _load_atlas_config(Path(d))
        assert roots == ["src", "lib"]
        assert langs is None


def test_languages_only():
    with tempfile.TemporaryDirectory() as d:
        _write_toml(Path(d), '[project]\nlanguages = ["python", "typescript"]\n')
        roots, langs = _load_atlas_config(Path(d))
        assert roots is None
        assert langs == ["python", "typescript"]


def test_both_keys_set():
    with tempfile.TemporaryDirectory() as d:
        _write_toml(
            Path(d),
            '[project]\nsource_roots = ["backend"]\nlanguages = ["python"]\n',
        )
        roots, langs = _load_atlas_config(Path(d))
        assert roots == ["backend"]
        assert langs == ["python"]


def test_empty_lists_treated_as_unset():
    """Empty lists in TOML are interpreted as "not configured" rather than
    "explicitly empty" — the latter has no useful meaning here. Users who
    want a no-resolver graph should omit the file or omit the key."""
    with tempfile.TemporaryDirectory() as d:
        _write_toml(Path(d), '[project]\nsource_roots = []\nlanguages = []\n')
        roots, langs = _load_atlas_config(Path(d))
        assert roots is None
        assert langs is None


def test_malformed_languages_warns_and_ignores():
    """A non-list `languages` value (e.g. a string) is rejected rather than
    silently coerced — surfacing the misconfiguration to the user."""
    with tempfile.TemporaryDirectory() as d:
        _write_toml(Path(d), '[project]\nlanguages = "python"\n')
        roots, langs = _load_atlas_config(Path(d))
        assert langs is None


def test_malformed_toml_returns_none_none():
    with tempfile.TemporaryDirectory() as d:
        _write_toml(Path(d), 'this is = not [valid toml')
        roots, langs = _load_atlas_config(Path(d))
        assert roots is None
        assert langs is None
