use semantic_engine::graph::RepoGraph;
use std::path::PathBuf;
use walkdir::WalkDir;

fn main() {
    let project_root = std::env::args()
        .nth(1)
        .expect("Usage: cargo run --example generate_map <project_path>");
    
    let root_path = PathBuf::from(&project_root);
    
    println!("Analyzing repository: {:?}\n", root_path);
    
    // Collect Python files
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
    
    // Calculate PageRank
    println!("Calculating PageRank...");
    graph.calculate_pagerank(20, 0.85);
    
    // Generate map
    println!("Generating repository map...\n");
    let map = graph.generate_map(10);
    
    println!("{}", map);
    
    // Save to file
    use std::fs;
    fs::write("repository_map.txt", &map).expect("Failed to write map to file");
    println!("\nMap saved to: repository_map.txt");
}