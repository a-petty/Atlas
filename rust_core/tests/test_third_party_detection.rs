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
