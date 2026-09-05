/// Tests for auto-detection of third-party packages from requirements.txt / pyproject.toml.

use semantic_engine::import_resolver::{ImportResolver, PythonImportResolver};
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn create_file(root: &Path, path: &str, content: &str) {
    let file_path = root.join(path);
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(file_path, content).unwrap();
}

#[test]
fn test_requirements_txt_packages_detected() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    create_file(root, "requirements.txt", "\
celery==5.3.4
redis>=4.0
httpx~=0.24
# a comment
-r other.txt

alembic
");
    // Create a local module that should still resolve
    create_file(root, "app/__init__.py", "");
    create_file(root, "app/models.py", "class User: pass");
    create_file(root, "main.py", "\
import celery
import redis
import httpx
import alembic
from app.models import User
");

    let resolver = PythonImportResolver::new(root, &[], None);

    // Third-party packages should be detected
    assert!(resolver.get_third_party_count() > 22, // more than just the hardcoded set
        "Expected more than hardcoded set, got {}", resolver.get_third_party_count());

    // Local module should still resolve
    let resolved = resolver.debug_module_lookup("app.models");
    assert!(resolved.is_some(), "app.models should still resolve");
}

#[test]
fn test_pyproject_toml_project_dependencies() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    create_file(root, "pyproject.toml", r#"
[project]
name = "myapp"
dependencies = [
    "fastapi>=0.100",
    "celery==5.3.4",
    "redis>=4.0",
    "python-jose[cryptography]",
]
"#);
    create_file(root, "app.py", "");

    let resolver = PythonImportResolver::new(root, &[], None);

    // fastapi is already hardcoded, but celery, redis, jose should be added
    assert!(resolver.get_third_party_count() > 22);
}

#[test]
fn test_pyproject_toml_poetry_dependencies() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    create_file(root, "pyproject.toml", r#"
[tool.poetry.dependencies]
python = "^3.11"
celery = "^5.3"
redis = "^4.0"
"#);
    create_file(root, "app.py", "");

    let resolver = PythonImportResolver::new(root, &[], None);
    assert!(resolver.get_third_party_count() > 22);
}

#[test]
fn test_known_mismatch_normalization() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    create_file(root, "requirements.txt", "\
python-jose
Pillow
scikit-learn
beautifulsoup4
pyyaml
psycopg2-binary
");
    create_file(root, "app.py", "");

    let resolver = PythonImportResolver::new(root, &[], None);
    let count = resolver.get_third_party_count();

    // These known mappings should all be present.
    // We can't directly check the set, but we can verify via the debug_resolve_import
    // that imports of these names are filtered as third-party.
    // The count should include jose, PIL, sklearn, bs4, yaml, psycopg2 plus the hardcoded set.
    assert!(count > 22, "Expected known mappings to be added, got {}", count);
}

#[test]
fn test_fallback_to_hardcoded_when_no_files() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // No requirements.txt or pyproject.toml
    create_file(root, "app.py", "import numpy");

    let resolver = PythonImportResolver::new(root, &[], None);
    // Should still have the hardcoded set
    assert!(resolver.get_third_party_count() >= 22,
        "Expected at least hardcoded set, got {}", resolver.get_third_party_count());
}

#[test]
fn test_subdirectory_requirements_txt() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    create_file(root, "backend/requirements.txt", "\
celery==5.3.4
redis>=4.0
");
    create_file(root, "app.py", "");

    let resolver = PythonImportResolver::new(root, &[], None);
    assert!(resolver.get_third_party_count() > 22,
        "Expected subdirectory packages to be found, got {}", resolver.get_third_party_count());
}

/// Depth-3 layout: `vendored/requirements/requirements-extra.txt`.
/// This is the FountainOfYouth llama.cpp shape — depth-1 walks miss it.
#[test]
fn test_depth_3_requirements_txt_detected() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    create_file(root, "vendored/requirements/requirements-extra.txt", "\
peft==0.5.0
trl>=0.7
");
    create_file(root, "app.py", "");

    let resolver = PythonImportResolver::new(root, &[], None);
    let baseline = baseline_third_party_count();
    assert!(
        resolver.get_third_party_count() > baseline,
        "Expected depth-3 requirements file to add packages: {} (baseline {})",
        resolver.get_third_party_count(),
        baseline,
    );
}

/// Depth-3 `pyproject.toml` (e.g., llama.cpp/gguf-py/pyproject.toml).
#[test]
fn test_depth_3_pyproject_toml_detected() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    create_file(root, "vendored/sub/pyproject.toml", r#"
[project]
name = "nested"
dependencies = ["peft", "trl"]
"#);
    create_file(root, "app.py", "");

    let resolver = PythonImportResolver::new(root, &[], None);
    let baseline = baseline_third_party_count();
    assert!(
        resolver.get_third_party_count() > baseline,
        "Expected depth-3 pyproject.toml to add packages: {} (baseline {})",
        resolver.get_third_party_count(),
        baseline,
    );
}

/// Boundary check: depth-4 requirements files are NOT walked. This bounds
/// the cost of a deep recursive walk on large projects with vendored trees.
/// `a/b/c/requirements.txt` has 4 path segments below root.
#[test]
fn test_depth_4_not_walked() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    create_file(root, "a/b/c/requirements.txt", "\
peft==0.5.0
trl>=0.7
");
    create_file(root, "app.py", "");

    let resolver = PythonImportResolver::new(root, &[], None);
    let baseline = baseline_third_party_count();
    assert_eq!(
        resolver.get_third_party_count(),
        baseline,
        "Depth-4 requirements.txt must not be discovered (got {}, baseline {})",
        resolver.get_third_party_count(),
        baseline,
    );
}

/// Vendored trees explicitly listed in `.gitignore` must be skipped during
/// dependency discovery — declarations there are typically transitive deps
/// of the vendored package, not first-party dependencies of this project.
///
/// The `ignore` crate default is `require_git=true`, so a `.git/` marker is
/// needed for `.gitignore` to take effect. In production this is the common
/// case (Atlas runs against repos); the marker here just simulates that.
#[test]
fn test_gitignored_vendored_tree_skipped() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    fs::create_dir_all(root.join(".git")).unwrap();
    create_file(root, ".gitignore", "vendored/\n");
    create_file(root, "vendored/requirements.txt", "\
peft==0.5.0
trl>=0.7
");
    create_file(root, "app.py", "");

    let resolver = PythonImportResolver::new(root, &[], None);
    let baseline = baseline_third_party_count();
    assert_eq!(
        resolver.get_third_party_count(),
        baseline,
        ".gitignored vendored tree must not contribute packages (got {}, baseline {})",
        resolver.get_third_party_count(),
        baseline,
    );
}

/// `node_modules` is in `DEFAULT_IGNORED_DIRS` and must always be skipped,
/// even with the deeper recursive walk. JS packages occasionally ship
/// `requirements.txt` for tooling — that should not pollute the Python
/// third-party set.
#[test]
fn test_node_modules_skipped() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    create_file(root, "node_modules/foo/requirements.txt", "\
peft==0.5.0
trl>=0.7
");
    create_file(root, "app.py", "");

    let resolver = PythonImportResolver::new(root, &[], None);
    let baseline = baseline_third_party_count();
    assert_eq!(
        resolver.get_third_party_count(),
        baseline,
        "node_modules must not be walked (got {}, baseline {})",
        resolver.get_third_party_count(),
        baseline,
    );
}

/// Builds a baseline by constructing a resolver in a fresh empty project,
/// so tests can assert deltas against the hardcoded fallback set without
/// needing to know its exact size.
fn baseline_third_party_count() -> usize {
    let dir = tempdir().unwrap();
    let root = dir.path();
    create_file(root, "app.py", "");
    PythonImportResolver::new(root, &[], None).get_third_party_count()
}
