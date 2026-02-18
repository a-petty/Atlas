use semantic_engine::graph::RepoGraph;
use std::fs;
use tempfile::tempdir;

fn create_test_file(root: &std::path::Path, path: &str, content: &str) {
    let file_path = root.join(path);
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(file_path, content).unwrap();
}

fn create_test_repo_with_structure() -> (tempfile::TempDir, RepoGraph) {
    let root = tempdir().unwrap();
    let root_path = root.path().to_path_buf();
    
    // Create a realistic project structure
    create_test_file(&root_path, "src/core.py", r#"
class Database:
    pass

class Config:
    pass
"#);
    
    create_test_file(&root_path, "src/models/user.py", r#"
from src.core import Database

class User:
    def __init__(self):
        self.db = Database()
"#);
    
    create_test_file(&root_path, "src/models/product.py", r#"
from src.core import Database

class Product:
    def __init__(self):
        self.db = Database()
"#);
    
    create_test_file(&root_path, "src/api/routes.py", r#"
from src.models.user import User
from src.models.product import Product

user = User()
product = Product()
"#);
    
    create_test_file(&root_path, "tests/test_api.py", r#"
from src.api.routes import user

def test_user():
    assert user is not None
"#);
    
    // Build graph
    let mut graph = RepoGraph::new(&root_path, "python", &[]);
    let paths = vec![
        root_path.join("src/core.py"),
        root_path.join("src/models/user.py"),
        root_path.join("src/models/product.py"),
        root_path.join("src/api/routes.py"),
        root_path.join("tests/test_api.py"),
    ];
    
    graph.build_complete(&paths, &root_path);
    
    // Calculate PageRank
    graph.calculate_pagerank(20, 0.85);
    
    (root, graph)
}

#[test]
fn test_generate_map_basic() {
    let (_root, mut graph) = create_test_repo_with_structure();
    
    // Generate map
    let map = graph.generate_map(5);
    
    println!("\n=== GENERATED MAP ===\n{}\n=====================\n", map);
    
    // Verify header
    assert!(map.contains("Repository Map (5 files)"));
    
    // Verify top ranked section exists
    assert!(map.contains("TOP RANKED FILES"));
    
    // Verify directory structure exists
    assert!(map.contains("DIRECTORY STRUCTURE:"));
    
    // Should contain core files (highest ranked)
    assert!(map.contains("src/core.py") || map.contains("core.py"));
}

#[test]
fn test_generate_map_includes_ranks() {
    let (_root, mut graph) = create_test_repo_with_structure();
    
    let map = graph.generate_map(5);
    
    // Should include rank scores
    assert!(map.contains("[rank:"));
    assert!(map.contains("imported by:"));
}

#[test]
fn test_generate_map_includes_stars() {
    let (_root, mut graph) = create_test_repo_with_structure();
    
    let map = graph.generate_map(5);
    
    // Should include star ratings
    assert!(map.contains("★"));
}

#[test]
fn test_generate_map_respects_max_files() {
    let (_root, mut graph) = create_test_repo_with_structure();
    
    let map_3 = graph.generate_map(3);
    let map_5 = graph.generate_map(5);
    
    // Map with max_files=3 should have fewer top ranked files listed
    let count_3 = map_3.matches("[rank:").count();
    let count_5 = map_5.matches("[rank:").count();
    
    assert!(count_3 <= 3, "Should list at most 3 top files");
    assert!(count_5 <= 5, "Should list at most 5 top files");
}

#[test]
fn test_generate_map_performance() {
    let (_root, mut graph) = create_test_repo_with_structure();
    
    use std::time::Instant;
    
    let start = Instant::now();
    let _map = graph.generate_map(10);
    let elapsed = start.elapsed();
    
    println!("Map generation took: {:?}", elapsed);
    
    // Should be fast (< 10ms target from roadmap)
    assert!(elapsed.as_millis() < 50, "Map generation took too long: {:?}", elapsed);
}

#[test]
fn test_generate_map_token_efficiency() {
    let (_root, mut graph) = create_test_repo_with_structure();
    
    let map = graph.generate_map(10);
    
    // Rough token count estimate (words + punctuation)
    let token_estimate = map.split_whitespace().count() + map.matches(|c: char| c.is_ascii_punctuation()).count();
    
    println!("Estimated tokens: {}", token_estimate);
    println!("Map length: {} chars", map.len());
    
    // Should be compact (< 500 tokens target for typical repos)
    // For this small test repo, should be much less
    assert!(token_estimate < 300, "Map is too verbose: {} tokens", token_estimate);
}