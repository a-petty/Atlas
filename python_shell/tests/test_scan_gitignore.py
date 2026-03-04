"""Test that scan_repository includes git-tracked files even if .gitignore matches them."""

import os
import subprocess
import tempfile

from atlas.semantic_engine import scan_repository


def test_force_added_ignored_file_is_included():
    """A file that matches .gitignore but is git-tracked should appear in scan results."""
    with tempfile.TemporaryDirectory() as tmp:
        # Set up a git repo
        subprocess.run(["git", "init"], cwd=tmp, check=True, capture_output=True)
        subprocess.run(
            ["git", "config", "user.email", "test@test.com"],
            cwd=tmp, check=True, capture_output=True,
        )
        subprocess.run(
            ["git", "config", "user.name", "Test"],
            cwd=tmp, check=True, capture_output=True,
        )

        # Create a directory with a Python file
        models_dir = os.path.join(tmp, "models")
        os.makedirs(models_dir)
        model_file = os.path.join(models_dir, "user.py")
        with open(model_file, "w") as f:
            f.write("class User:\n    pass\n")

        # Create .gitignore that excludes models/
        with open(os.path.join(tmp, ".gitignore"), "w") as f:
            f.write("models/\n")

        # Force-add the file so git tracks it despite .gitignore
        subprocess.run(["git", "add", "-f", model_file], cwd=tmp, check=True, capture_output=True)
        subprocess.run(["git", "add", ".gitignore"], cwd=tmp, check=True, capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "initial"],
            cwd=tmp, check=True, capture_output=True,
        )

        # Scan and check
        files = scan_repository(tmp)
        canonical = os.path.realpath(model_file)
        assert canonical in files, (
            f"Expected {canonical} in scan results but got: {files}"
        )


def test_normal_ignored_file_is_excluded():
    """A file matching .gitignore that is NOT git-tracked should still be excluded."""
    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run(["git", "init"], cwd=tmp, check=True, capture_output=True)
        subprocess.run(
            ["git", "config", "user.email", "test@test.com"],
            cwd=tmp, check=True, capture_output=True,
        )
        subprocess.run(
            ["git", "config", "user.name", "Test"],
            cwd=tmp, check=True, capture_output=True,
        )

        # Create file + .gitignore but do NOT git-add the file
        os.makedirs(os.path.join(tmp, "build"))
        build_file = os.path.join(tmp, "build", "output.py")
        with open(build_file, "w") as f:
            f.write("x = 1\n")

        with open(os.path.join(tmp, ".gitignore"), "w") as f:
            f.write("build/\n")

        subprocess.run(["git", "add", ".gitignore"], cwd=tmp, check=True, capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "initial"],
            cwd=tmp, check=True, capture_output=True,
        )

        files = scan_repository(tmp)
        canonical = os.path.realpath(build_file)
        assert canonical not in files, (
            f"Expected {canonical} NOT in scan results but it was found"
        )
