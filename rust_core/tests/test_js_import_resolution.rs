use semantic_engine::graph::RepoGraph;
use std::path::PathBuf;
use petgraph::visit::EdgeRef;

#[test]
fn test_js_import_resolution() {
    let test_repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/js_test_repo");
    
    let mut graph = RepoGraph::new(&test_repo, "javascript", &[], None);
    let paths: Vec<PathBuf> = vec![
        test_repo.join("src/index.js"),
        test_repo.join("src/utils/helpers.js"),
        test_repo.join("src/components/Button.tsx"),
    ];
    graph.build_complete(&paths, &test_repo);
    
    // Verify files were indexed
    assert!(graph.path_to_idx.len() > 0);
    
    // Verify edges were created
    let edge_count = graph.graph.edge_count();
    assert!(edge_count > 0, "Expected edges but got zero");
}

#[test]
fn test_ts_path_alias_resolution() {
    let test_repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/js_test_repo");
    
    let mut graph = RepoGraph::new(&test_repo, "typescript", &[], None);
    let paths: Vec<PathBuf> = vec![
        test_repo.join("src/index.js"),
        test_repo.join("src/utils/helpers.js"),
        test_repo.join("src/components/Button.tsx"),
    ];
    graph.build_complete(&paths, &test_repo);
    
    let index_js_path = test_repo.join("src/index.js");
    let button_tsx_path = test_repo.join("src/components/Button.tsx");

    let outgoing = graph.get_outgoing_dependencies(&index_js_path);
    assert!(outgoing.iter().any(|(p, _)| p == &button_tsx_path));
}
