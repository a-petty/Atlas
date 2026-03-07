//! Test to reproduce the CPG build hang on phase_1_high_level.py
use semantic_engine::cpg::CpgLayer;
use semantic_engine::parser::SupportedLanguage;
use std::path::Path;
use std::time::{Duration, Instant};
use tree_sitter::Parser as TreeSitterParser;

#[test]
fn test_phase1_high_level_cpg_build() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("phase_1_high_level.py");

    if !path.exists() {
        eprintln!("Skipping: {} not found", path.display());
        return;
    }

    let source = std::fs::read_to_string(&path).unwrap();
    let lang = SupportedLanguage::Python;
    let ts_lang = lang.get_parser().unwrap();

    let mut parser = TreeSitterParser::new();
    parser.set_language(ts_lang).unwrap();
    let tree = parser.parse(&source, None).unwrap();

    let mut cpg = CpgLayer::new();

    let start = Instant::now();
    cpg.build_file(&path, tree, source, lang);
    let elapsed = start.elapsed();

    eprintln!("build_file completed in {:.2}s", elapsed.as_secs_f64());
    eprintln!("Graph: {} nodes, {} edges", cpg.graph.node_count(), cpg.graph.edge_count());

    assert!(
        elapsed < Duration::from_secs(10),
        "build_file took {:.1}s — should complete in under 10s",
        elapsed.as_secs_f64()
    );
}
