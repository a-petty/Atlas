use semantic_engine::graph::{RepoGraph, EdgeKind};
use std::path::PathBuf;
use tempfile::tempdir;

fn create_test_graph() -> RepoGraph {
    let root = tempdir().unwrap();
    let root_path = root.path();
    let mut graph = RepoGraph::new(root_path, "python", &[]);

    // Add files (nodes) with initial rank 0.0
    let core_path = PathBuf::from("src/core.py");
    let util1_path = PathBuf::from("src/util1.py");
    let util2_path = PathBuf::from("src/util2.py");
    let app_path = PathBuf::from("src/app.py");
    let leaf_path = PathBuf::from("src/leaf.py");

    graph.add_file(core_path.clone(), "").unwrap();
    graph.add_file(util1_path.clone(), "").unwrap();
    graph.add_file(util2_path.clone(), "").unwrap();
    graph.add_file(app_path.clone(), "").unwrap();
    graph.add_file(leaf_path.clone(), "").unwrap();

    let core_idx = graph.path_to_idx[&core_path];
    let util1_idx = graph.path_to_idx[&util1_path];
    let util2_idx = graph.path_to_idx[&util2_path];
    let app_idx = graph.path_to_idx[&app_path];
    let leaf_idx = graph.path_to_idx[&leaf_path];


    // Everyone depends on core.py
    // app -> core (SymbolUsage, strong)
    graph.graph.add_edge(app_idx, core_idx, EdgeKind::SymbolUsage);
    // util1 -> core (Import, weak)
    graph.graph.add_edge(util1_idx, core_idx, EdgeKind::Import);
    // util2 -> core (Import, weak)
    graph.graph.add_edge(util2_idx, core_idx, EdgeKind::Import);
    
    // app also depends on util1
    graph.graph.add_edge(app_idx, util1_idx, EdgeKind::SymbolUsage);
    
    // util2 depends on leaf
    graph.graph.add_edge(util2_idx, leaf_idx, EdgeKind::SymbolUsage);

    graph
}

#[test]
fn test_pagerank_identifies_core_file() {
    let mut graph = create_test_graph();
    
    // Act
    graph.calculate_pagerank(20, 0.85);
    
    let top_files = graph.get_top_ranked_files(5);

    // Assert
    // core.py should be #1 because it has the most incoming links
    assert_eq!(top_files[0].0, PathBuf::from("src/core.py"));

    // util1.py should be #2 because it's used by app.py
    assert_eq!(top_files[1].0, PathBuf::from("src/util1.py"));
    
    // app.py should be higher than util2.py and leaf.py
    let app_rank = top_files.iter().find(|(p, _)| p == &PathBuf::from("src/app.py")).unwrap().1;
    let util2_rank = top_files.iter().find(|(p, _)| p == &PathBuf::from("src/util2.py")).unwrap().1;
    let leaf_rank = top_files.iter().find(|(p, _)| p == &PathBuf::from("src/leaf.py")).unwrap().1;

    assert_eq!(app_rank, util2_rank);
    assert!(leaf_rank > util2_rank);
}

#[test]
fn test_pagerank_handles_empty_graph() {
    let root = tempdir().unwrap();
    let root_path = root.path();
    let mut graph = RepoGraph::new(root_path, "python", &[]);
    graph.calculate_pagerank(20, 0.85);
    let top_files = graph.get_top_ranked_files(1);
    assert!(top_files.is_empty());
}