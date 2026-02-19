// rust_core/tests/test_graph_updates.rs

use semantic_engine::graph::{RepoGraph, EdgeKind};
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use std::fs;

// Helper to create a dummy RepoGraph with a pre-defined file structure
fn setup_test_repo_graph() -> (tempfile::TempDir, RepoGraph) {
    let dir = tempdir().unwrap();
    let root = dir.path();
    
    // Create dummy files
    fs::create_dir_all(root.join("src/models")).unwrap();
    fs::write(root.join("src/main.py"), "import src.auth\nimport src.utils").unwrap();
    fs::write(root.join("src/auth.py"), "from .models import user").unwrap();
    fs::write(root.join("src/utils.py"), "# No imports").unwrap();
    fs::write(root.join("src/models/user.py"), "# Defines User model").unwrap();

    let files_to_scan = vec![
        root.join("src/main.py"),
        root.join("src/auth.py"),
        root.join("src/utils.py"),
        root.join("src/models/user.py"),
    ];

    let mut graph = RepoGraph::new(root, "python", &[], None);
    graph.build_complete(&files_to_scan, root);

    (dir, graph)
}

#[test]
fn test_update_file_add_import() {
    let (_dir, mut graph) = setup_test_repo_graph();
    let utils_path = graph.project_root.join("src/utils.py");
    let auth_path = graph.project_root.join("src/auth.py");
    
    // Modify utils.py to add an import
    fs::write(&utils_path, "import src.auth").unwrap();

    // Perform the update
    let content = fs::read_to_string(&utils_path).unwrap();
    let result = graph.update_file(&utils_path, &content).unwrap();

    // Verify
    let utils_deps: Vec<_> = graph.get_outgoing_dependencies(&utils_path).into_iter().map(|(p, _)| p).collect();
    assert_eq!(utils_deps.len(), 1);
    assert!(utils_deps.contains(&auth_path));
    assert!(result.edges_added >= 1);
    assert!(result.needs_pagerank_recalc);
}

#[test]
fn test_update_file_remove_import() {
    let (_dir, mut graph) = setup_test_repo_graph();
    let main_path = graph.project_root.join("src/main.py");
    let utils_path = graph.project_root.join("src/utils.py");

    // Verify initial state
    let initial_deps: Vec<_> = graph.get_outgoing_dependencies(&main_path).into_iter().map(|(p, _)| p).collect();
    assert!(initial_deps.contains(&utils_path));
    
    // Modify main.py to remove an import
    fs::write(&main_path, "import src.auth").unwrap();

    // Perform the update
    let content = fs::read_to_string(&main_path).unwrap();
    let result = graph.update_file(&main_path, &content).unwrap();

    // Verify
    let final_deps: Vec<_> = graph.get_outgoing_dependencies(&main_path).into_iter().map(|(p, _)| p).collect();
    assert!(!final_deps.contains(&utils_path));
    assert_eq!(result.edges_removed, 1);
    assert!(result.needs_pagerank_recalc);
}

#[test]
fn test_update_file_no_change() {
    let (_dir, mut graph) = setup_test_repo_graph();
    let main_path = graph.project_root.join("src/main.py");
    
    // Get original content
    let original_content = fs::read_to_string(&main_path).unwrap();

    // Write the same content back
    fs::write(&main_path, &original_content).unwrap();

    // Perform the update
    let result = graph.update_file(&main_path, &original_content).unwrap();

    // Verify
    assert_eq!(result.edges_added, 0);
    assert_eq!(result.edges_removed, 0);
    assert!(!result.needs_pagerank_recalc);
}

#[test]
fn test_lazy_pagerank_recalculation() {
    let (_dir, mut graph) = setup_test_repo_graph();
    let main_path = graph.project_root.join("src/main.py");

    // Initial state: pagerank should be clean after build_complete
    assert!(!graph.pagerank_dirty);

    // Modify file to trigger an update
    fs::write(&main_path, "import src.utils").unwrap();
    let content = fs::read_to_string(&main_path).unwrap();
    let result = graph.update_file(&main_path, &content).unwrap();

    // An edge was changed, so pagerank should be dirty
    assert!(result.needs_pagerank_recalc);
    assert!(graph.pagerank_dirty);

    // Accessing a rank-dependent method should clean the flag
    let _ = graph.get_top_ranked_files(1);
    assert!(!graph.pagerank_dirty);
}

#[test]
fn test_update_performance() {
    let dir = tempdir().unwrap();
    let root = &dir.path().canonicalize().unwrap();
    let mut graph = RepoGraph::new(root, "python", &[], None);
    
    let num_files = 500;
    let mut file_paths = Vec::new();

    // Setup a large graph
    for i in 0..num_files {
        let path = root.join(format!("src/file_{}.py", i));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "").unwrap();
        file_paths.push(path);
    }
    graph.build_complete(&file_paths, root);

    // Create some initial connections
    for i in 0..num_files {
        let target_idx = (i + 1) % num_files;
        let content = format!("import src.file_{}", target_idx);
        fs::write(&file_paths[i], &content).unwrap();
        graph.update_file(&file_paths[i], &content).unwrap();
    }
    graph.pagerank_dirty = false; // reset dirty flag

    let file_to_update = file_paths[0].clone();
    let new_content = "import src.file_2\nimport src.file_3";
    fs::write(&file_to_update, &new_content).unwrap();

    let start = std::time::Instant::now();
    let _ = graph.update_file(&file_to_update, &new_content).unwrap();
    let duration = start.elapsed();

    println!("\nGraph update for one file in a {}-node graph took: {:?}", num_files, duration);
    // Success criteria is < 10ms. This test will verify it's in the right ballpark.
    assert!(duration < std::time::Duration::from_millis(10));
}

#[test]
fn test_symbol_definition_removal() {
    // SETUP
    let dir = tempdir().unwrap();
    let root = &dir.path().canonicalize().unwrap();
    
    // Create file structure
    fs::create_dir_all(root.join("src")).unwrap();
    
    // File A defines func_a
    fs::write(
        root.join("src/module_a.py"),
        "def func_a():\n    pass"
    ).unwrap();
    
    // File B calls func_a
    fs::write(
        root.join("src/module_b.py"),
        "from src.module_a import func_a\nfunc_a()"
    ).unwrap();
    
    let files = vec![
        root.join("src/module_a.py"),
        root.join("src/module_b.py"),
    ];
    
    let mut graph = RepoGraph::new(root, "python", &[], None);
    graph.build_complete(&files, root);
    
    // VERIFY: Initial state has Import edge B -> A (B imports from A)
    let module_a = root.join("src/module_a.py");
    let module_b = root.join("src/module_b.py");

    let initial_edges = graph.get_outgoing_dependencies(&module_b);
    assert!(
        initial_edges.iter().any(|(path, _kind)| {
            path == &module_a
        }),
        "Expected edge from B to A"
    );

    // ACTION: Rename func_a to func_renamed in File A
    fs::write(
        root.join("src/module_a.py"),
        "def func_renamed():\n    pass"
    ).unwrap();

    let content = fs::read_to_string(&module_a).unwrap();
    let result = graph.update_file(&module_a, &content).unwrap();

    // ASSERT: Import edge B -> A still exists (B's import statement is unchanged)
    // The SymbolUsage match for func_a is gone, but the Import edge persists
    let final_edges = graph.get_outgoing_dependencies(&module_b);
    assert!(
        final_edges.iter().any(|(path, kind)| {
            path == &module_a && *kind == EdgeKind::Import
        }),
        "Import edge should persist after definition rename (B still imports from A)"
    );
    
    // ASSERT: SymbolIndex no longer contains func_a from File A
    assert!(
        !graph.symbol_index.definitions.contains_key("func_a"),
        "func_a should no longer be in symbol index"
    );
}

#[test]
fn test_symbol_definition_addition() {
    let dir = tempdir().unwrap();
    let root = &dir.path().canonicalize().unwrap();
    
    fs::create_dir_all(root.join("src")).unwrap();
    
    // File A initially empty
    fs::write(root.join("src/module_a.py"), "# Empty file").unwrap();
    
    // File B calls func_a (currently undefined)
    fs::write(
        root.join("src/module_b.py"),
        "from src.module_a import func_a\nfunc_a()"
    ).unwrap();
    
    let files = vec![
        root.join("src/module_a.py"),
        root.join("src/module_b.py"),
    ];
    
    let mut graph = RepoGraph::new(root, "python", &[], None);
    graph.build_complete(&files, root);
    
    let module_a = root.join("src/module_a.py");
    let module_b = root.join("src/module_b.py");
    
    // VERIFY: Import edge exists from build (B imports from A via `from src.module_a import func_a`)
    let initial_edges = graph.get_outgoing_dependencies(&module_b);
    assert!(
        initial_edges.iter().any(|(path, kind)| {
            path == &module_a && *kind == EdgeKind::Import
        }),
        "Import edge should exist from B to A (from build-time import resolution)"
    );

    // ACTION: Add func_a definition to File A
    fs::write(
        root.join("src/module_a.py"),
        "def func_a():\n    pass"
    ).unwrap();

    let content = fs::read_to_string(&module_a).unwrap();
    let _result = graph.update_file(&module_a, &content).unwrap();

    // ASSERT: Edge B -> A still exists (Import edge persists)
    let final_edges = graph.get_outgoing_dependencies(&module_b);
    assert!(
        final_edges.iter().any(|(path, _kind)| {
            path == &module_a
        }),
        "Edge B -> A should still exist after definition added"
    );
}

#[test]
fn test_symbol_usage_change() {
    let dir = tempdir().unwrap();
    let root = &dir.path().canonicalize().unwrap();
    
    fs::create_dir_all(root.join("src")).unwrap();
    
    // File A defines func_a
    fs::write(
        root.join("src/module_a.py"),
        "def func_a():\n    pass"
    ).unwrap();
    
    // File C defines func_c
    fs::write(
        root.join("src/module_c.py"),
        "def func_c():\n    pass"
    ).unwrap();
    
    // File B initially calls func_a
    fs::write(
        root.join("src/module_b.py"),
        "from src.module_a import func_a\nfunc_a()"
    ).unwrap();
    
    let files = vec![
        root.join("src/module_a.py"),
        root.join("src/module_b.py"),
        root.join("src/module_c.py"),
    ];
    
    let mut graph = RepoGraph::new(root, "python", &[], None);
    graph.build_complete(&files, root);
    
    let module_a = root.join("src/module_a.py");
    let module_b = root.join("src/module_b.py");
    let module_c = root.join("src/module_c.py");
    
    // VERIFY: Edge B -> A exists (Import from `from src.module_a import func_a`)
    let initial_edges = graph.get_outgoing_dependencies(&module_b);
    assert!(
        initial_edges.iter().any(|(path, _kind)| {
            path == &module_a
        }),
        "Expected initial edge B -> A"
    );

    // ACTION: Change File B to call func_c instead
    fs::write(
        root.join("src/module_b.py"),
        "from src.module_c import func_c\nfunc_c()"
    ).unwrap();

    let content = fs::read_to_string(&module_b).unwrap();
    let result = graph.update_file(&module_b, &content).unwrap();

    // ASSERT: Edge B -> A is removed (import changed)
    let final_edges = graph.get_outgoing_dependencies(&module_b);
    assert!(
        !final_edges.iter().any(|(path, _kind)| {
            path == &module_a
        }),
        "Old edge B -> A should be removed"
    );

    // ASSERT: New edge B -> C is created (new import)
    assert!(
        final_edges.iter().any(|(path, _kind)| {
            path == &module_c
        }),
        "New edge B -> C should be created"
    );

    assert!(result.edges_added > 0);
    assert!(result.edges_removed > 0);
    assert!(result.needs_pagerank_recalc);
}

#[test]
fn test_symbol_collision_handling() {
    let dir = tempdir().unwrap();
    let root = &dir.path().canonicalize().unwrap();

    fs::create_dir_all(root.join("src")).unwrap();

    // File A defines func_common
    fs::write(
        root.join("src/module_a.py"),
        "def func_common():\n    pass"
    ).unwrap();

    // File C also defines func_common (collision)
    fs::write(
        root.join("src/module_c.py"),
        "def func_common():\n    pass"
    ).unwrap();

    // File B uses func_common WITHOUT importing — should NOT create edges
    fs::write(
        root.join("src/module_b.py"),
        "func_common()"
    ).unwrap();

    let files = vec![
        root.join("src/module_a.py"),
        root.join("src/module_b.py"),
        root.join("src/module_c.py"),
    ];

    let mut graph = RepoGraph::new(root, "python", &[], None);
    graph.build_complete(&files, root);

    let module_a = root.join("src/module_a.py");
    let module_b = root.join("src/module_b.py");
    let module_c = root.join("src/module_c.py");

    // VERIFY: B has NO SymbolUsage edges (no imports → no edges)
    // This is import-aware filtering: name coincidence alone doesn't create edges
    let edges = graph.get_outgoing_dependencies(&module_b);
    let symbol_edges: Vec<_> = edges.iter()
        .filter(|(_, kind)| *kind == EdgeKind::SymbolUsage)
        .collect();

    assert!(
        symbol_edges.is_empty(),
        "B should have no SymbolUsage edges without imports (got {:?})", symbol_edges
    );

    // VERIFY: SymbolIndex still tracks both definitions (the index is unaffected)
    let defs = graph.symbol_index.definitions.get("func_common").unwrap();
    assert_eq!(defs.len(), 2, "Should track both definitions");
    assert!(defs.contains(&module_a));
    assert!(defs.contains(&module_c));
}

#[test]
fn test_file_creation_with_symbols() {
    let dir = tempdir().unwrap();
    let root = &dir.path().canonicalize().unwrap();
    
    fs::create_dir_all(root.join("src")).unwrap();
    
    // Existing file with a function
    fs::write(
        root.join("src/old_file.py"),
        "def old_func():\n    pass"
    ).unwrap();
    
    let files = vec![root.join("src/old_file.py")];
    
    let mut graph = RepoGraph::new(root, "python", &[], None);
    graph.build_complete(&files, root);
    
    let old_file = root.join("src/old_file.py");
    let new_file = root.join("src/new_file.py");
    
    // ACTION: Create new file that uses old_func and defines new_func
    fs::write(
        &new_file,
        "from src.old_file import old_func\ndef new_func():\n    old_func()"
    ).unwrap();
    
    // Simulate file creation by adding it to the graph
    // Note: This would typically be done through a higher-level API
    graph.build(&[new_file.clone()]);
    graph.build_semantic_edges();
    
    // VERIFY: New node exists
    assert!(graph.path_to_idx.contains_key(&new_file));
    
    // VERIFY: Edge from new_file -> old_file exists
    let edges = graph.get_outgoing_dependencies(&new_file);
    assert!(
        edges.iter().any(|(path, _)| path == &old_file),
        "Should have edge to old_file"
    );
    
    // VERIFY: Symbol index updated
    let defs = graph.symbol_index.definitions.get("new_func").unwrap();
    assert!(defs.contains(&new_file));
}

#[test]
fn test_file_deletion_cleanup() {
    let dir = tempdir().unwrap();
    let root = &dir.path().canonicalize().unwrap();
    
    fs::create_dir_all(root.join("src")).unwrap();
    
    // File A defines func_a
    fs::write(
        root.join("src/module_a.py"),
        "def func_a():\n    pass"
    ).unwrap();
    
    // File B calls func_a
    fs::write(
        root.join("src/module_b.py"),
        "from src.module_a import func_a\nfunc_a()"
    ).unwrap();
    
    let files = vec![
        root.join("src/module_a.py"),
        root.join("src/module_b.py"),
    ];
    
    let mut graph = RepoGraph::new(root, "python", &[], None);
    graph.build_complete(&files, root);
    
    let _module_b = root.join("src/module_b.py");
    
    // VERIFY: Initial state
    let _initial_node_count = graph.graph.node_count();
    let _initial_edge_count = graph.graph.edge_count();
    
    // ACTION: Delete File B
    // This would need a new method: graph.remove_file(&module_b)
    // For now, test the conceptual requirement
    
    // FUTURE: Implement graph.remove_file method
    // let result = graph.remove_file(&module_b).unwrap();
    
    // EXPECTED ASSERTIONS (once implemented):
    // assert!(!graph.path_to_idx.contains_key(&module_b));
    // assert!(graph.graph.node_count() == initial_node_count - 1);
    // assert!(graph.graph.edge_count() < initial_edge_count);
}

#[test]
fn test_update_maintains_consistency() {
    let (_dir, mut graph) = setup_test_repo_graph();
    
    // Initial state should be consistent
    graph.validate_consistency().expect("Initial graph should be consistent");
    
    // Perform multiple updates
    let files_to_update = vec![
        graph.project_root.join("src/main.py"),
        graph.project_root.join("src/auth.py"),
        graph.project_root.join("src/utils.py"),
    ];
    
    for file in &files_to_update {
        // Make some change
        let new_content = "# Modified content\nimport src.auth";
        fs::write(file, new_content).unwrap();
        
        graph.update_file(file, new_content).unwrap();
        
        // After each update, graph should remain consistent
        graph.validate_consistency().expect(&format!(
            "Graph inconsistent after updating {}",
            file.display()
        ));
    }
}

#[test]
fn test_batch_updates_maintain_consistency() {
    let dir = tempdir().unwrap();
    let root = &dir.path().canonicalize().unwrap();
    
    // Create a complex graph
    fs::create_dir_all(root.join("src")).unwrap();
    for i in 0..20 {
        fs::write(
            root.join(format!("src/module_{}.py", i)),
            format!("def func_{}():\n    pass", i)
        ).unwrap();
    }
    
    let files: Vec<_> = (0..20)
        .map(|i| root.join(format!("src/module_{}.py", i)))
        .collect();
    
    let mut graph = RepoGraph::new(root, "python", &[], None);
    graph.build_complete(&files, root);
    
    graph.validate_consistency().expect("Initial build should be consistent");
    
    // Simulate rapid file changes
    for i in 0..20 {
        let file = root.join(format!("src/module_{}.py", i));
        let target = (i + 1) % 20;
        let content = format!("from src.module_{} import func_{}\ndef func_{}():\n    func_{}()", target, target, i, target);
        fs::write(&file, &content).unwrap();
        
        graph.update_file(&file, &content).unwrap();
    }
    
    graph.validate_consistency().expect("Graph should remain consistent after batch updates");
}