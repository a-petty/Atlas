use semantic_engine::graph::RepoGraph;
use std::path::PathBuf;
use walkdir::WalkDir;

fn main() {
    let project_root = std::env::args()
        .nth(1)
        .expect("Usage: cargo run --example test_graph_build <project_path>");
    
    let root_path = PathBuf::from(&project_root);
    
    println!("Building graph for: {:?}", root_path);
    
    // Collect all Python files
    let mut paths = Vec::new();
    for entry in WalkDir::new(&root_path)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("py"))
    {
        paths.push(entry.path().to_path_buf());
    }
    
    println!("Found {} Python files", paths.len());
    
    // Build graph
    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    graph.build_complete(&paths, &root_path);
    
    // Print statistics
    let stats = graph.get_statistics();
    println!("\n=== Graph Statistics ===");
    println!("Nodes: {}", stats.node_count);
    println!("Total Edges: {}", stats.edge_count);
    println!("  Import Edges: {}", stats.import_edges);
    println!("  Symbol Edges: {}", stats.symbol_edges);
    println!("Unique Symbols Defined: {}", stats.total_definitions);
    println!("Files with Symbol Usages: {}", stats.total_files_with_usages);
}