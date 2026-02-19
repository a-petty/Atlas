/// Integration test: verifies that the full scan_repository → build_complete flow
/// correctly resolves absolute imports when paths are canonicalized.
///
/// This catches the bug where `scan_repository` canonicalizes paths (e.g.,
/// /var/folders/... → /private/var/folders/... on macOS) but the import
/// resolver's module_index stored non-canonical paths, causing silent edge
/// creation failures.

use semantic_engine::graph::RepoGraph;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn create_file(root: &Path, path: &str, content: &str) {
    let file_path = root.join(path);
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(file_path, content).unwrap();
}

/// Simulate what scan_repository does: walk and canonicalize paths.
fn scan_and_canonicalize(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
    {
        if let Some(ext) = entry.path().extension().and_then(|s| s.to_str()) {
            if ext == "py" {
                if let Ok(canonical) = entry.path().canonicalize() {
                    files.push(canonical);
                }
            }
        }
    }
    files
}

#[test]
fn test_absolute_import_with_canonical_paths() {
    let root = tempdir().unwrap();
    // Use the NON-canonical path (like MCP server might pass before resolve())
    let non_canonical_root = root.path().to_path_buf();

    // Django-like project
    create_file(&non_canonical_root, "backend/app/__init__.py", "");
    create_file(&non_canonical_root, "backend/app/models/__init__.py", "");
    create_file(
        &non_canonical_root,
        "backend/app/models/enums.py",
        "class PlanStatus: pass",
    );
    create_file(&non_canonical_root, "backend/app/views/__init__.py", "");
    create_file(
        &non_canonical_root,
        "backend/app/views/api.py",
        "from app.models.enums import PlanStatus",
    );
    create_file(&non_canonical_root, "backend/manage.py", "");

    // Simulate production: scan_repository canonicalizes paths
    let canonical_files = scan_and_canonicalize(&non_canonical_root);
    assert!(
        canonical_files.len() >= 5,
        "Should find at least 5 Python files, found {}",
        canonical_files.len()
    );

    // Build graph with NON-canonical root (RepoGraph::new should canonicalize internally)
    let mut graph = RepoGraph::new(&non_canonical_root, "python", &[], None);
    graph.build_complete(&canonical_files, &non_canonical_root);

    // The canonical root is what the graph uses internally
    let canonical_root = non_canonical_root.canonicalize().unwrap();

    // Verify both files are in the graph
    let api_path = canonical_root.join("backend/app/views/api.py");
    let enums_path = canonical_root.join("backend/app/models/enums.py");

    assert!(
        graph.has_file(&api_path),
        "api.py should be in graph at {:?}",
        api_path
    );
    assert!(
        graph.has_file(&enums_path),
        "enums.py should be in graph at {:?}",
        enums_path
    );

    // KEY ASSERTION: absolute import edge should exist
    let deps = graph.get_outgoing_dependencies(&api_path);
    let dep_paths: HashSet<PathBuf> = deps.iter().map(|(p, _)| p.clone()).collect();

    assert!(
        dep_paths.contains(&enums_path),
        "api.py should have import edge to enums.py.\n\
         Dependencies found: {:?}\n\
         Expected: {:?}\n\
         Unresolved imports: {:?}",
        dep_paths,
        enums_path,
        graph.get_unresolved_imports_sample(10),
    );

    // Verify no unresolved imports for this case
    let stats = graph.get_statistics();
    assert_eq!(
        stats.unresolved_import_count, 0,
        "All imports should be resolved, but {} are unresolved.\n\
         Source roots: {:?}\n\
         Module index size: {}",
        stats.unresolved_import_count,
        stats.source_roots,
        stats.module_index_size,
    );
}

#[test]
fn test_relative_import_with_canonical_paths() {
    let root = tempdir().unwrap();
    let non_canonical_root = root.path().to_path_buf();

    create_file(&non_canonical_root, "pkg/__init__.py", "");
    create_file(&non_canonical_root, "pkg/utils.py", "def helper(): pass");
    create_file(
        &non_canonical_root,
        "pkg/main.py",
        "from . import utils",
    );

    let canonical_files = scan_and_canonicalize(&non_canonical_root);
    let mut graph = RepoGraph::new(&non_canonical_root, "python", &[], None);
    graph.build_complete(&canonical_files, &non_canonical_root);

    let canonical_root = non_canonical_root.canonicalize().unwrap();
    let main_path = canonical_root.join("pkg/main.py");
    let utils_path = canonical_root.join("pkg/utils.py");

    let deps = graph.get_outgoing_dependencies(&main_path);
    let dep_paths: HashSet<PathBuf> = deps.iter().map(|(p, _)| p.clone()).collect();

    assert!(
        dep_paths.contains(&utils_path),
        "main.py should have import edge to utils.py via relative import.\n\
         Dependencies found: {:?}",
        dep_paths,
    );
}

#[test]
fn test_source_roots_detected_in_statistics() {
    let root = tempdir().unwrap();
    let non_canonical_root = root.path().to_path_buf();

    create_file(&non_canonical_root, "backend/app/__init__.py", "");
    create_file(&non_canonical_root, "backend/app/core.py", "");

    let canonical_files = scan_and_canonicalize(&non_canonical_root);
    let mut graph = RepoGraph::new(&non_canonical_root, "python", &[], None);
    graph.build_complete(&canonical_files, &non_canonical_root);

    let stats = graph.get_statistics();
    assert_eq!(
        stats.source_roots.len(),
        1,
        "Should detect backend/ as a source root"
    );
    let canonical_root = non_canonical_root.canonicalize().unwrap();
    assert_eq!(
        stats.source_roots[0],
        canonical_root.join("backend"),
        "Source root should be canonical path to backend/"
    );
    assert!(
        stats.module_index_size > 0,
        "Module index should have entries"
    );
}
