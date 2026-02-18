use semantic_engine::graph::{RepoGraph, EdgeKind};
use std::fs;
use tempfile::tempdir;

fn create_test_file(root: &std::path::Path, path: &str, content: &str) {
    let file_path = root.join(path);
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(file_path, content).unwrap();
}

#[test]
fn test_import_edge_creation() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    
    // Create a simple project
    create_test_file(&root_path, "utils.py", r#"
def helper():
    pass
"#);
    
    create_test_file(&root_path, "main.py", r#"
import utils

utils.helper()
"#);
    
    // Build graph
    let mut graph = RepoGraph::new(&root_path, "python", &[]);
    let paths = vec![
        root_path.join("utils.py"),
        root_path.join("main.py"),
    ];
    
    graph.build_complete(&paths, &root_path);
    
    // Verify import edge exists
    assert_eq!(graph.graph.node_count(), 2, "Should have 2 nodes");
    assert!(graph.graph.edge_count() >= 1, "Should have at least 1 edge");
    
    // Check that main.py imports utils.py
    let main_idx = graph.path_to_idx.get(&root_path.join("main.py")).unwrap();
    let utils_idx = graph.path_to_idx.get(&root_path.join("utils.py")).unwrap();
    
    let edge = graph.graph.find_edge(*main_idx, *utils_idx);
    assert!(edge.is_some(), "Should have edge from main to utils");
}

#[test]
fn test_symbol_usage_edge_creation() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    
    // Create files with symbol usage
    create_test_file(&root_path, "models.py", r#"
class User:
    pass
"#);
    
    create_test_file(&root_path, "app.py", r#"
from models import User

user = User()
"#);
    
    let mut graph = RepoGraph::new(&root_path, "python", &[]);
    let paths = vec![
        root_path.join("models.py"),
        root_path.join("app.py"),
    ];
    
    graph.build_complete(&paths, &root_path);
    
    // Verify both import and symbol edges
    assert_eq!(graph.graph.node_count(), 2);
    assert!(graph.graph.edge_count() >= 1);
    
    // When both Import and SymbolUsage edges exist for the same pair,
    // Import wins because it's structurally confirmed via AST analysis
    let app_idx = graph.path_to_idx.get(&root_path.join("app.py")).unwrap();
    let models_idx = graph.path_to_idx.get(&root_path.join("models.py")).unwrap();

    let edge_idx = graph.graph.find_edge(*app_idx, *models_idx).unwrap();
    let edge_kind = &graph.graph[edge_idx];

    assert_eq!(edge_kind, &EdgeKind::Import, "Import should take priority over SymbolUsage");
}

#[test]
fn test_no_self_edges() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    
    create_test_file(&root_path, "self_import.py", r#"
class MyClass:
    pass

obj = MyClass()
"#);
    
    let mut graph = RepoGraph::new(&root_path, "python", &[]);
    let paths = vec![root_path.join("self_import.py")];
    
    graph.build_complete(&paths, &root_path);
    
    assert_eq!(graph.graph.node_count(), 1);
    assert_eq!(graph.graph.edge_count(), 0, "Should have no self-edges");
}

#[test]
fn test_multiple_symbol_definitions() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    
    // Same symbol defined in two files
    create_test_file(&root_path, "util1.py", r#"
def process():
    pass
"#);
    
    create_test_file(&root_path, "util2.py", r#"
def process():
    pass
"#);
    
    create_test_file(&root_path, "main.py", r#"
from util1 import process

process()
"#);
    
    let mut graph = RepoGraph::new(&root_path, "python", &[]);
    let paths = vec![
        root_path.join("util1.py"),
        root_path.join("util2.py"),
        root_path.join("main.py"),
    ];
    
    graph.build_complete(&paths, &root_path);
    
    // Should have 3 nodes
    assert_eq!(graph.graph.node_count(), 3);
    
    // Should have edges to util1 (not util2, since we import from util1)
    let main_idx = graph.path_to_idx.get(&root_path.join("main.py")).unwrap();
    let util1_idx = graph.path_to_idx.get(&root_path.join("util1.py")).unwrap();
    
    assert!(graph.graph.find_edge(*main_idx, *util1_idx).is_some());
}

#[test]
fn test_graph_statistics() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    
    // Create a small project
    create_test_file(&root_path, "core.py", r#"
class Database:
    pass
"#);
    
    create_test_file(&root_path, "models.py", r#"
from core import Database

class User:
    def __init__(self):
        self.db = Database()
"#);
    
    create_test_file(&root_path, "api.py", r#"
from models import User

user = User()
"#);
    
    let mut graph = RepoGraph::new(&root_path, "python", &[]);
    let paths = vec![
        root_path.join("core.py"),
        root_path.join("models.py"),
        root_path.join("api.py"),
    ];
    
    graph.build_complete(&paths, &root_path);
    
    println!("Nodes: {}", graph.graph.node_count());
    println!("Edges: {}", graph.graph.edge_count());
    
    // Should have 3 nodes
    assert_eq!(graph.graph.node_count(), 3);
    
    // Should have at least 2 edges (models->core, api->models)
    assert!(graph.graph.edge_count() >= 2);
}