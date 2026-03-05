/// Integration tests for __init__.py re-export following.
///
/// When `from app.models import User` resolves to `models/__init__.py`,
/// and that `__init__.py` has `from .user import User`, we should get
/// edges to BOTH `__init__.py` AND `user.py`.

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

fn scan_py_files(root: &Path) -> Vec<PathBuf> {
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

fn get_dep_paths(graph: &RepoGraph, file: &Path) -> HashSet<PathBuf> {
    graph
        .get_outgoing_dependencies(file)
        .into_iter()
        .map(|(p, _)| p)
        .collect()
}

#[test]
fn test_barrel_reexport_creates_edge_to_submodule() {
    let root = tempdir().unwrap();
    let r = root.path();

    create_file(r, "app/__init__.py", "");
    create_file(r, "app/models/__init__.py", "from .user import User\n");
    create_file(r, "app/models/user.py", "class User: pass\n");
    create_file(r, "app/views.py", "from app.models import User\n");

    let canonical_root = r.canonicalize().unwrap();
    let files = scan_py_files(r);
    let mut graph = RepoGraph::new(r, "python", &[], None);
    graph.build_complete(&files, r);

    let views_path = canonical_root.join("app/views.py");
    let init_path = canonical_root.join("app/models/__init__.py");
    let user_path = canonical_root.join("app/models/user.py");

    let deps = get_dep_paths(&graph, &views_path);

    assert!(
        deps.contains(&init_path),
        "Should have edge to __init__.py, got: {:?}",
        deps
    );
    assert!(
        deps.contains(&user_path),
        "Should have edge to user.py via re-export, got: {:?}",
        deps
    );
}

#[test]
fn test_multiple_reexports_from_init() {
    let root = tempdir().unwrap();
    let r = root.path();

    create_file(r, "app/__init__.py", "");
    create_file(
        r,
        "app/models/__init__.py",
        "from .user import User\nfrom .post import Post\n",
    );
    create_file(r, "app/models/user.py", "class User: pass\n");
    create_file(r, "app/models/post.py", "class Post: pass\n");
    create_file(r, "app/views.py", "from app.models import User, Post\n");

    let canonical_root = r.canonicalize().unwrap();
    let files = scan_py_files(r);
    let mut graph = RepoGraph::new(r, "python", &[], None);
    graph.build_complete(&files, r);

    let views_path = canonical_root.join("app/views.py");
    let user_path = canonical_root.join("app/models/user.py");
    let post_path = canonical_root.join("app/models/post.py");

    let deps = get_dep_paths(&graph, &views_path);

    assert!(
        deps.contains(&user_path),
        "Should have edge to user.py, got: {:?}",
        deps
    );
    assert!(
        deps.contains(&post_path),
        "Should have edge to post.py, got: {:?}",
        deps
    );
}

#[test]
fn test_non_init_import_unchanged() {
    let root = tempdir().unwrap();
    let r = root.path();

    create_file(r, "app/__init__.py", "");
    create_file(r, "app/utils.py", "def helper(): pass\n");
    create_file(r, "app/main.py", "from app.utils import helper\n");

    let canonical_root = r.canonicalize().unwrap();
    let files = scan_py_files(r);
    let mut graph = RepoGraph::new(r, "python", &[], None);
    graph.build_complete(&files, r);

    let main_path = canonical_root.join("app/main.py");
    let utils_path = canonical_root.join("app/utils.py");

    let deps = get_dep_paths(&graph, &main_path);

    assert!(
        deps.contains(&utils_path),
        "Should have edge to utils.py, got: {:?}",
        deps
    );
}

/// Mimics FountainOfYouth layout: backend/ source root with app/ package.
/// `from app.models.ai_inbox import ...` should resolve to backend/app/models/ai_inbox.py.
/// `from app.models import AiInbox` (via __init__.py re-export) should also resolve.
#[test]
fn test_backend_source_root_with_init_reexports() {
    let root = tempdir().unwrap();
    let r = root.path();

    // FOY-like structure
    create_file(r, "backend/app/__init__.py", "");
    create_file(
        r,
        "backend/app/models/__init__.py",
        "from .ai_inbox import AiInboxItem\nfrom .plan import Plan\n",
    );
    create_file(r, "backend/app/models/ai_inbox.py", "class AiInboxItem: pass\n");
    create_file(r, "backend/app/models/plan.py", "class Plan: pass\n");
    create_file(
        r,
        "backend/app/services/planner.py",
        "from app.models.ai_inbox import AiInboxItem\nfrom app.models import Plan\n",
    );
    create_file(r, "backend/app/services/__init__.py", "");

    let canonical_root = r.canonicalize().unwrap();
    let files = scan_py_files(r);
    let mut graph = RepoGraph::new(r, "python", &[], None);
    graph.build_complete(&files, r);

    let planner = canonical_root.join("backend/app/services/planner.py");
    let ai_inbox = canonical_root.join("backend/app/models/ai_inbox.py");
    let plan = canonical_root.join("backend/app/models/plan.py");
    let models_init = canonical_root.join("backend/app/models/__init__.py");

    let deps = get_dep_paths(&graph, &planner);

    // Direct import: from app.models.ai_inbox import AiInboxItem
    assert!(
        deps.contains(&ai_inbox),
        "Should resolve direct import to ai_inbox.py, got: {:?}",
        deps
    );

    // __init__.py re-export: from app.models import Plan → models/__init__.py → plan.py
    assert!(
        deps.contains(&models_init),
        "Should have edge to models/__init__.py, got: {:?}",
        deps
    );
    assert!(
        deps.contains(&plan),
        "Should follow __init__.py re-export to plan.py, got: {:?}",
        deps
    );
}

/// Test that unresolved stats reflect reality — resolved imports shouldn't be counted as unresolved.
#[test]
fn test_no_unresolved_for_valid_reexports() {
    let root = tempdir().unwrap();
    let r = root.path();

    create_file(r, "app/__init__.py", "");
    create_file(r, "app/models/__init__.py", "from .user import User\n");
    create_file(r, "app/models/user.py", "class User: pass\n");
    create_file(r, "app/views.py", "from app.models import User\n");

    let files = scan_py_files(r);
    let mut graph = RepoGraph::new(r, "python", &[], None);
    graph.build_complete(&files, r);

    let stats = graph.get_statistics();
    assert_eq!(
        stats.unresolved_import_count, 0,
        "All imports should resolve, but {} unresolved. Sample: {:?}",
        stats.unresolved_import_count,
        graph.get_unresolved_imports_sample(10),
    );
}
