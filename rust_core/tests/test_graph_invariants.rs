use semantic_engine::graph::{EdgeKind, RepoGraph};
use semantic_engine::import_resolver::ResolverGroup;
use std::fs;
use tempfile::tempdir;

#[test]
fn typed_evidence_survives_rebuild_and_tracks_import_removal() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let a = root.join("a.py");
    let b = root.join("b.py");
    fs::write(&a, "from b import work\nwork()\n").unwrap();
    fs::write(&b, "def work():\n    pass\n").unwrap();
    let mut graph = RepoGraph::new_multi(&root, &[ResolverGroup::Python], &[], None);
    graph.build_complete(&[a.clone(), b.clone()], &root);
    for _ in 0..2 {
        let edges = graph.get_dependencies(&a);
        assert_eq!(edges.iter().filter(|(p, k)| p == &b && *k == EdgeKind::Import).count(), 1);
        assert_eq!(edges.iter().filter(|(p, k)| p == &b && *k == EdgeKind::SymbolUsage).count(), 1);
        graph.build_semantic_edges();
    }
    // Keep the same name use; removing its import removes both kinds of evidence.
    graph.update_file(&a, "work()\n").unwrap();
    assert!(graph.get_dependencies(&a).is_empty());
    graph.update_file(&a, "from b import work\nwork()\n").unwrap();
    assert_eq!(graph.get_dependencies(&a).len(), 2);
}

#[test]
fn skeleton_uses_accepted_buffer_and_invalidates_after_update() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let path = root.join("a.py");
    fs::write(&path, "def original():\n    pass\n").unwrap();
    let mut graph = RepoGraph::new_multi(&root, &[ResolverGroup::Python], &[], None);
    graph.build_complete(&[path.clone()], &root);
    assert!(graph.get_skeleton(&path).unwrap().contains("original"));
    graph.update_file(&path, "def accepted():\n    pass\n").unwrap();
    fs::write(&path, "def racing_disk_edit():\n    pass\n").unwrap();
    let skeleton = graph.get_skeleton(&path).unwrap();
    assert!(skeleton.contains("accepted"));
    assert!(!skeleton.contains("original") && !skeleton.contains("racing_disk_edit"));
}

#[test]
fn pagerank_conserves_mass_with_dangling_nodes() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut graph = RepoGraph::new_multi(&root, &[ResolverGroup::Python], &[], None);
    graph.add_file(root.join("a.py"), "def a():\n    pass\n").unwrap();
    graph.add_file(root.join("b.py"), "def b():\n    pass\n").unwrap();
    graph.calculate_pagerank(40, 0.85);
    let sum: f64 = graph.graph.node_weights().map(|n| n.rank).sum();
    assert!((sum - 1.0).abs() < 1e-12, "rank mass was {sum}");
}

#[test]
fn incremental_create_delete_recreate_matches_fresh_typed_graph() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let a = root.join("a.py");
    let b = root.join("future.py");
    fs::write(&a, "from future import work\ndef caller(): return work()\n").unwrap();
    let mut graph = RepoGraph::new_multi(&root, &[ResolverGroup::Python], &[], None);
    graph.build_complete(&[a.clone()], &root);
    for round in 0..12 {
        let source = format!("def work(): return {round}\n");
        fs::write(&b, &source).unwrap();
        graph.add_file(b.clone(), &source).unwrap();
        let mut fresh = RepoGraph::new_multi(&root, &[ResolverGroup::Python], &[], None);
        fresh.build_complete(&[a.clone(), b.clone()], &root);
        let signature = |g: &RepoGraph| {
            let mut edges: Vec<_> = g.get_dependencies(&a).into_iter()
                .map(|(path, kind)| (path, format!("{kind:?}"))).collect();
            edges.sort(); edges
        };
        assert_eq!(signature(&graph), signature(&fresh));
        assert_eq!(signature(&graph).len(), 2);
        graph.validate_consistency().unwrap();
        fs::remove_file(&b).unwrap();
        graph.remove_file(&b).unwrap();
        assert!(graph.get_dependencies(&a).is_empty());
        graph.validate_consistency().unwrap();
    }
}

#[test]
fn local_binding_changes_match_fresh_edges_and_invalidate_rank() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let a = root.join("a.py");
    let b = root.join("b.py");
    let original = "from b import foo\ndef run():\n    foo()\n";
    let shadowed = "from b import foo\ndef run(foo):\n    foo()\n";
    fs::write(&a, original).unwrap();
    fs::write(&b, "def foo(): pass\n").unwrap();
    let mut graph = RepoGraph::new_multi(&root, &[ResolverGroup::Python], &[], None);
    graph.build_complete(&[a.clone(), b.clone()], &root);
    let signature = |g: &RepoGraph| {
        let mut edges: Vec<_> = g.get_dependencies(&a).into_iter()
            .map(|(path, kind)| (path, format!("{kind:?}"))).collect();
        edges.sort(); edges
    };
    for source in [shadowed, original, shadowed, original] {
        fs::write(&a, source).unwrap();
        let result = graph.update_file(&a, source).unwrap();
        let mut fresh = RepoGraph::new_multi(&root, &[ResolverGroup::Python], &[], None);
        fresh.build_complete(&[a.clone(), b.clone()], &root);
        assert_eq!(signature(&graph), signature(&fresh));
        assert!(result.needs_pagerank_recalc);
        assert_eq!(result.edges_added + result.edges_removed, 1);
        graph.validate_consistency().unwrap();
    }
    let result = graph.update_file(&a, "from b import foo\ndef run():\n    # body-only edit\n    foo()\n").unwrap();
    assert!(!result.needs_pagerank_recalc);
    assert_eq!(result.edges_added + result.edges_removed, 0);
}

#[test]
fn local_js_export_changes_refresh_incoming_semantic_evidence() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let a = root.join("main.js");
    let b = root.join("lib.js");
    fs::write(root.join("package.json"), "{}").unwrap();
    fs::write(&a, "import {foo} from './lib'; function run(){foo();}").unwrap();
    fs::write(&b, "export function foo(){}").unwrap();
    let mut graph = RepoGraph::new_multi(&root, &[ResolverGroup::JsTs], &[], None);
    graph.build_complete(&[a.clone(), b.clone()], &root);
    for (source, expected) in [("function foo(){}", 1), ("export function foo(){}", 2)] {
        fs::write(&b, source).unwrap();
        let result = graph.update_file(&b, source).unwrap();
        assert_eq!(graph.get_dependencies(&a).len(), expected);
        assert!(result.needs_pagerank_recalc);
        assert_eq!(result.edges_added + result.edges_removed, 1);
    }
}
