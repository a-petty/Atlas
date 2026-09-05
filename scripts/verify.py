#!/usr/bin/env python3
"""Build the extension from current Rust sources, then verify both language layers."""
from pathlib import Path
import argparse
import os
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def run(command, **kwargs):
    print("+ " + " ".join(map(str, command)), flush=True)
    subprocess.run(command, cwd=ROOT, check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", action="store_true", help="Use the optimized extension for integration tests")
    parser.add_argument("--skip-lock-check", action="store_true", help="For environments without uv; recorded output is not a locked install check")
    args = parser.parse_args()
    env = dict(os.environ)
    env["VIRTUAL_ENV"] = sys.prefix
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    if not args.skip_lock_check:
        run(["uv", "lock", "--check"], env=env)
    run(["cargo", "test", "--locked", "--manifest-path", "rust_core/Cargo.toml", "--", "--test-threads=1"], env=env)
    command = [sys.executable, "-m", "maturin", "develop"]
    if args.release:
        command.append("--release")
    run(command, env=env)
    run([sys.executable, "-m", "pytest", "python_shell/tests", "-q", "-p", "no:cacheprovider"], env=env)


if __name__ == "__main__":
    main()
