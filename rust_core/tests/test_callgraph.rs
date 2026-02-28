use semantic_engine::callgraph::{CallGraphBuilder, CallSite};
use semantic_engine::cpg::{CpgEdge, CpgLayer, CpgNodeKind, StatementKind};
use semantic_engine::graph::RepoGraph;
use semantic_engine::symbol_table::SymbolIndex;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;
use tree_sitter::Parser as TreeSitterParser;
use semantic_engine::parser::SupportedLanguage;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_cpg_for_source(cpg: &mut CpgLayer, path: &str, source: &str) {
    let path = PathBuf::from(path);
    let mut parser = TreeSitterParser::new();
    parser
        .set_language(SupportedLanguage::Python.get_parser().unwrap())
        .unwrap();
    let tree = parser.parse(source, None).unwrap();
    cpg.build_file(&path, tree, source.to_string(), SupportedLanguage::Python);
}

fn find_function(cpg: &CpgLayer, path: &str, name: &str) -> Option<NodeIndex> {
    let path = PathBuf::from(path);
    cpg.file_to_nodes.get(&path)?.iter().find(|idx| {
        cpg.graph
            .node_weight(**idx)
            .map(|n| {
                matches!(n.kind, CpgNodeKind::Function | CpgNodeKind::Method)
                    && n.name == name
            })
            .unwrap_or(false)
    }).copied()
}

fn create_test_file(root: &std::path::Path, path: &str, content: &str) {
    let file_path = root.join(path);
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(file_path, content).unwrap();
}

fn has_calls_edge(cpg: &CpgLayer, from: NodeIndex, to: NodeIndex) -> bool {
    cpg.graph
        .edges_directed(from, petgraph::Direction::Outgoing)
        .any(|e| e.target() == to && *e.weight() == CpgEdge::Calls)
}

fn has_called_by_edge(cpg: &CpgLayer, from: NodeIndex, to: NodeIndex) -> bool {
    cpg.graph
        .edges_directed(from, petgraph::Direction::Outgoing)
        .any(|e| e.target() == to && *e.weight() == CpgEdge::CalledBy)
}

fn count_edge_kind(cpg: &CpgLayer, kind_matcher: impl Fn(&CpgEdge) -> bool) -> usize {
    cpg.graph.edge_indices()
        .filter(|&e| kind_matcher(&cpg.graph[e]))
        .count()
}

fn count_calls_edges(cpg: &CpgLayer, from: NodeIndex, to: NodeIndex) -> usize {
    cpg.graph
        .edges_connecting(from, to)
        .filter(|e| *e.weight() == CpgEdge::Calls)
        .count()
}

fn count_called_by_edges(cpg: &CpgLayer, from: NodeIndex, to: NodeIndex) -> usize {
    cpg.graph
        .edges_connecting(from, to)
        .filter(|e| *e.weight() == CpgEdge::CalledBy)
        .count()
}

fn get_dataflow_arg_edges(cpg: &CpgLayer) -> Vec<(NodeIndex, NodeIndex, usize)> {
    cpg.graph.edge_indices()
        .filter_map(|e| {
            if let CpgEdge::DataFlowArgument { position } = &cpg.graph[e] {
                let (src, tgt) = cpg.graph.edge_endpoints(e).unwrap();
                Some((src, tgt, *position))
            } else {
                None
            }
        })
        .collect()
}

fn get_dataflow_return_edges(cpg: &CpgLayer) -> Vec<(NodeIndex, NodeIndex)> {
    cpg.graph.edge_indices()
        .filter_map(|e| {
            if *cpg.graph.edge_weight(e).unwrap() == CpgEdge::DataFlowReturn {
                cpg.graph.edge_endpoints(e)
            } else {
                None
            }
        })
        .collect()
}

// ============================================================
// Call Site Extraction Tests (1-7)
// ============================================================

// Test 1: Simple call: g() → callee_name="g", no args
#[test]
fn test_extract_simple_call() {
    let mut cpg = CpgLayer::new();
    let source = "def f():\n    g()\n";
    build_cpg_for_source(&mut cpg, "/test/t1.py", source);

    let func_idx = find_function(&cpg, "/test/t1.py", "f").unwrap();
    let sites = cpg.call_sites.get(&func_idx).unwrap();

    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].callee_name, "g");
    assert!(sites[0].receiver.is_none());
    assert!(sites[0].positional_args.is_empty());
    assert!(sites[0].keyword_args.is_empty());
}

// Test 2: Call with args: g(x, 1) → positional_args=["x", "1"]
#[test]
fn test_extract_call_with_args() {
    let mut cpg = CpgLayer::new();
    let source = "def f(x):\n    g(x, 1)\n";
    build_cpg_for_source(&mut cpg, "/test/t2.py", source);

    let func_idx = find_function(&cpg, "/test/t2.py", "f").unwrap();
    let sites = cpg.call_sites.get(&func_idx).unwrap();

    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].callee_name, "g");
    assert_eq!(sites[0].positional_args, vec!["x", "1"]);
}

// Test 3: Call with kwargs: g(x=1, y=2)
#[test]
fn test_extract_call_with_kwargs() {
    let mut cpg = CpgLayer::new();
    let source = "def f():\n    g(x=1, y=2)\n";
    build_cpg_for_source(&mut cpg, "/test/t3.py", source);

    let func_idx = find_function(&cpg, "/test/t3.py", "f").unwrap();
    let sites = cpg.call_sites.get(&func_idx).unwrap();

    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].callee_name, "g");
    assert!(sites[0].positional_args.is_empty());
    assert_eq!(sites[0].keyword_args.len(), 2);
    assert_eq!(sites[0].keyword_args[0], ("x".to_string(), "1".to_string()));
    assert_eq!(sites[0].keyword_args[1], ("y".to_string(), "2".to_string()));
}

// Test 4: Method call: self.bar() → callee_name="bar", receiver="self"
#[test]
fn test_extract_method_call() {
    let mut cpg = CpgLayer::new();
    let source = "class C:\n    def foo(self):\n        self.bar()\n";
    build_cpg_for_source(&mut cpg, "/test/t4.py", source);

    let func_idx = find_function(&cpg, "/test/t4.py", "foo").unwrap();
    let sites = cpg.call_sites.get(&func_idx).unwrap();

    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].callee_name, "bar");
    assert_eq!(sites[0].receiver.as_deref(), Some("self"));
}

// Test 5: Nested calls: g(h(1)) → two call sites
#[test]
fn test_extract_nested_calls() {
    let mut cpg = CpgLayer::new();
    let source = "def f():\n    g(h(1))\n";
    build_cpg_for_source(&mut cpg, "/test/t5.py", source);

    let func_idx = find_function(&cpg, "/test/t5.py", "f").unwrap();
    let sites = cpg.call_sites.get(&func_idx).unwrap();

    assert_eq!(sites.len(), 2, "Should extract both g() and h() calls");
    let names: HashSet<&str> = sites.iter().map(|s| s.callee_name.as_str()).collect();
    assert!(names.contains("g"));
    assert!(names.contains("h"));
}

// Test 6: No calls: x = 1 → empty
#[test]
fn test_extract_no_calls() {
    let mut cpg = CpgLayer::new();
    let source = "def f():\n    x = 1\n";
    build_cpg_for_source(&mut cpg, "/test/t6.py", source);

    let func_idx = find_function(&cpg, "/test/t6.py", "f").unwrap();
    let sites = cpg.call_sites.get(&func_idx).unwrap();

    assert!(sites.is_empty());
}

// Test 7: Chained method call: obj.m1().m2() → extracts method calls
#[test]
fn test_extract_chained_calls() {
    let mut cpg = CpgLayer::new();
    let source = "def f(obj):\n    obj.m1().m2()\n";
    build_cpg_for_source(&mut cpg, "/test/t7.py", source);

    let func_idx = find_function(&cpg, "/test/t7.py", "f").unwrap();
    let sites = cpg.call_sites.get(&func_idx).unwrap();

    // Should find at least m1 and m2 calls
    assert!(sites.len() >= 2, "Should extract chained calls, got {}", sites.len());
    let names: HashSet<&str> = sites.iter().map(|s| s.callee_name.as_str()).collect();
    assert!(names.contains("m1"));
    assert!(names.contains("m2"));
}

// ============================================================
// Call Resolution Tests (8-12)
// ============================================================

// Test 8: Same-file resolution: f() calls g() in same file → Calls/CalledBy edges
#[test]
fn test_resolve_same_file() {
    let mut cpg = CpgLayer::new();
    let source = "def g():\n    pass\n\ndef f():\n    g()\n";
    build_cpg_for_source(&mut cpg, "/test/t8.py", source);

    let symbol_index = SymbolIndex::new();
    CallGraphBuilder::resolve_all(&mut cpg, &symbol_index);

    let f_idx = find_function(&cpg, "/test/t8.py", "f").unwrap();
    let g_idx = find_function(&cpg, "/test/t8.py", "g").unwrap();

    assert!(has_calls_edge(&cpg, f_idx, g_idx), "f should have Calls edge to g");
    assert!(has_called_by_edge(&cpg, g_idx, f_idx), "g should have CalledBy edge to f");
}

// Test 9: Cross-file resolution via symbol index
#[test]
fn test_resolve_cross_file() {
    let mut cpg = CpgLayer::new();
    let source_a = "def caller():\n    target()\n";
    let source_b = "def target():\n    pass\n";
    build_cpg_for_source(&mut cpg, "/test/a.py", source_a);
    build_cpg_for_source(&mut cpg, "/test/b.py", source_b);

    // Set up symbol index to know that `target` is defined in b.py
    let mut symbol_index = SymbolIndex::new();
    symbol_index.definitions.insert(
        "target".to_string(),
        vec![PathBuf::from("/test/b.py")],
    );

    CallGraphBuilder::resolve_all(&mut cpg, &symbol_index);

    let caller_idx = find_function(&cpg, "/test/a.py", "caller").unwrap();
    let target_idx = find_function(&cpg, "/test/b.py", "target").unwrap();

    assert!(has_calls_edge(&cpg, caller_idx, target_idx), "caller should call target");
    assert!(has_called_by_edge(&cpg, target_idx, caller_idx), "target should be called by caller");
}

// Test 10: Self-method resolution: self.b() resolves to C.b
#[test]
fn test_resolve_self_method() {
    let mut cpg = CpgLayer::new();
    let source = "class C:\n    def a(self):\n        self.b()\n\n    def b(self):\n        pass\n";
    build_cpg_for_source(&mut cpg, "/test/t10.py", source);

    let symbol_index = SymbolIndex::new();
    CallGraphBuilder::resolve_all(&mut cpg, &symbol_index);

    let a_idx = find_function(&cpg, "/test/t10.py", "a").unwrap();
    let b_idx = find_function(&cpg, "/test/t10.py", "b").unwrap();

    assert!(has_calls_edge(&cpg, a_idx, b_idx), "a should call b via self");
}

// Test 11: Builtin unresolved: print(x) → no edges
#[test]
fn test_builtin_unresolved() {
    let mut cpg = CpgLayer::new();
    let source = "def f(x):\n    print(x)\n";
    build_cpg_for_source(&mut cpg, "/test/t11.py", source);

    let symbol_index = SymbolIndex::new();
    CallGraphBuilder::resolve_all(&mut cpg, &symbol_index);

    let calls_count = count_edge_kind(&cpg, |e| *e == CpgEdge::Calls);
    assert_eq!(calls_count, 0, "print() should not create Calls edges");
}

// Test 12: Ambiguous unresolved: two functions with same name → no edges
#[test]
fn test_ambiguous_unresolved() {
    let mut cpg = CpgLayer::new();
    // Two files each define 'process'
    let source_a = "def process():\n    pass\n";
    let source_b = "def process():\n    pass\n";
    let source_c = "def caller():\n    process()\n";
    build_cpg_for_source(&mut cpg, "/test/a.py", source_a);
    build_cpg_for_source(&mut cpg, "/test/b.py", source_b);
    build_cpg_for_source(&mut cpg, "/test/c.py", source_c);

    // Symbol index says 'process' is defined in both a.py and b.py
    let mut symbol_index = SymbolIndex::new();
    symbol_index.definitions.insert(
        "process".to_string(),
        vec![PathBuf::from("/test/a.py"), PathBuf::from("/test/b.py")],
    );

    CallGraphBuilder::resolve_all(&mut cpg, &symbol_index);

    let calls_count = count_edge_kind(&cpg, |e| *e == CpgEdge::Calls);
    assert_eq!(calls_count, 0, "Ambiguous call should not resolve");
}

// ============================================================
// Argument/Return Flow Tests (13-15)
// ============================================================

// Test 13: Positional args: callee(x, y) / def callee(a, b) → DataFlowArgument at pos 0, 1
#[test]
fn test_positional_arg_flow() {
    let mut cpg = CpgLayer::new();
    let source = "def callee(a, b):\n    return a + b\n\ndef caller():\n    callee(1, 2)\n";
    build_cpg_for_source(&mut cpg, "/test/t13.py", source);

    let symbol_index = SymbolIndex::new();
    CallGraphBuilder::resolve_all(&mut cpg, &symbol_index);

    let arg_edges = get_dataflow_arg_edges(&cpg);
    // Should have position 0 and position 1
    let positions: HashSet<usize> = arg_edges.iter().map(|(_, _, pos)| *pos).collect();
    assert!(positions.contains(&0), "Should have DataFlowArgument at position 0");
    assert!(positions.contains(&1), "Should have DataFlowArgument at position 1");
}

// Test 14: Keyword args: callee(b=1, a=2) → correct position mapping
#[test]
fn test_keyword_arg_flow() {
    let mut cpg = CpgLayer::new();
    let source = "def callee(a, b):\n    return a + b\n\ndef caller():\n    callee(b=1, a=2)\n";
    build_cpg_for_source(&mut cpg, "/test/t14.py", source);

    let symbol_index = SymbolIndex::new();
    CallGraphBuilder::resolve_all(&mut cpg, &symbol_index);

    let arg_edges = get_dataflow_arg_edges(&cpg);
    let positions: HashSet<usize> = arg_edges.iter().map(|(_, _, pos)| *pos).collect();
    // Both args should map: a → position 0, b → position 1
    assert!(positions.contains(&0), "kwarg 'a' should map to position 0");
    assert!(positions.contains(&1), "kwarg 'b' should map to position 1");
}

// Test 15: Return flow: return x in callee → DataFlowReturn to caller's call stmt
#[test]
fn test_return_flow() {
    let mut cpg = CpgLayer::new();
    let source = "def callee():\n    return 42\n\ndef caller():\n    x = callee()\n";
    build_cpg_for_source(&mut cpg, "/test/t15.py", source);

    let symbol_index = SymbolIndex::new();
    CallGraphBuilder::resolve_all(&mut cpg, &symbol_index);

    let return_edges = get_dataflow_return_edges(&cpg);
    assert!(!return_edges.is_empty(), "Should have DataFlowReturn edges");

    // The source should be a return statement, the target should be a call-containing statement
    for (src, _tgt) in &return_edges {
        let src_node = cpg.graph.node_weight(*src).unwrap();
        assert_eq!(
            src_node.statement_kind,
            Some(StatementKind::Return),
            "DataFlowReturn source should be a return statement"
        );
    }
}

// ============================================================
// Integration Tests (16-18)
// ============================================================

// Test 16: RepoGraph full integration: enable_cpg, build_complete, verify call graph
#[test]
fn test_repograph_call_graph_integration() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "main.py", r#"from helpers import add

def main():
    result = add(1, 2)
    return result
"#);
    create_test_file(&root_path, "helpers.py", r#"def add(a, b):
    return a + b
"#);

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    graph.enable_cpg();
    let paths = vec![root_path.join("main.py"), root_path.join("helpers.py")];
    graph.build_complete(&paths, &root_path);

    let cpg = graph.cpg.as_ref().unwrap();

    let main_idx = find_function(cpg, &root_path.join("main.py").to_string_lossy(), "main").unwrap();
    let add_idx = find_function(cpg, &root_path.join("helpers.py").to_string_lossy(), "add").unwrap();

    assert!(has_calls_edge(cpg, main_idx, add_idx), "main should call add");
    assert!(has_called_by_edge(cpg, add_idx, main_idx), "add should be called by main");

    // Also verify argument flow edges exist
    let arg_edges = get_dataflow_arg_edges(cpg);
    assert!(!arg_edges.is_empty(), "Should have argument flow edges");
}

// Test 17: Incremental update: add new call → new edges appear
#[test]
fn test_incremental_update_adds_call_edges() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "funcs.py", "def g():\n    pass\n\ndef f():\n    pass\n");

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    graph.enable_cpg();
    let paths = vec![root_path.join("funcs.py")];
    graph.build_complete(&paths, &root_path);

    // Initially no call edges
    let cpg = graph.cpg.as_ref().unwrap();
    let calls_before = count_edge_kind(cpg, |e| *e == CpgEdge::Calls);
    assert_eq!(calls_before, 0);

    // Update f to call g
    let new_source = "def g():\n    pass\n\ndef f():\n    g()\n";
    fs::write(root_path.join("funcs.py"), new_source).unwrap();
    let funcs_path = root_path.join("funcs.py");
    graph.update_file(&funcs_path, new_source).unwrap();

    let cpg = graph.cpg.as_ref().unwrap();
    let calls_after = count_edge_kind(cpg, |e| *e == CpgEdge::Calls);
    assert!(calls_after > 0, "Should have Calls edges after update, got {}", calls_after);
}

// Test 18: Remove file: remove callee file → no dangling edges
#[test]
fn test_remove_file_no_dangling_edges() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "a.py", "def f():\n    g()\n");
    create_test_file(&root_path, "b.py", "def g():\n    pass\n");

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    graph.enable_cpg();
    let paths = vec![root_path.join("a.py"), root_path.join("b.py")];
    graph.build_complete(&paths, &root_path);

    // Verify call edge exists
    let cpg = graph.cpg.as_ref().unwrap();
    let calls_before = count_edge_kind(cpg, |e| *e == CpgEdge::Calls);
    assert!(calls_before > 0, "Should have Calls edge before removal");

    // Remove b.py
    graph.remove_file(&root_path.join("b.py")).unwrap();

    // Verify no dangling edges
    let cpg = graph.cpg.as_ref().unwrap();
    for edge_idx in cpg.graph.edge_indices() {
        let (src, tgt) = cpg.graph.edge_endpoints(edge_idx).unwrap();
        assert!(
            cpg.graph.node_weight(src).is_some(),
            "Edge source {:?} has no node", src
        );
        assert!(
            cpg.graph.node_weight(tgt).is_some(),
            "Edge target {:?} has no node", tgt
        );
    }
}

// Test 19: Self-method attribute filter: self.x = val is not a call
#[test]
fn test_self_attribute_not_call() {
    let mut cpg = CpgLayer::new();
    let source = "class C:\n    def m(self, val):\n        self.x = val\n";
    build_cpg_for_source(&mut cpg, "/test/t19.py", source);

    let func_idx = find_function(&cpg, "/test/t19.py", "m").unwrap();
    let sites = cpg.call_sites.get(&func_idx).unwrap();

    // self.x = val is an assignment, not a call
    assert!(sites.is_empty(), "self.x = val should not be extracted as a call");
}

// ============================================================
// Edge Cases (20-21)
// ============================================================

// Test 20: Recursive call: def f(): f() → self-referencing Calls edge
#[test]
fn test_recursive_call() {
    let mut cpg = CpgLayer::new();
    let source = "def f():\n    f()\n";
    build_cpg_for_source(&mut cpg, "/test/t20.py", source);

    let symbol_index = SymbolIndex::new();
    CallGraphBuilder::resolve_all(&mut cpg, &symbol_index);

    let f_idx = find_function(&cpg, "/test/t20.py", "f").unwrap();
    assert!(has_calls_edge(&cpg, f_idx, f_idx), "f should have self-referencing Calls edge");
}

// Test 21: Method self-param skip: arg 0 maps to param after self/cls
#[test]
fn test_self_param_skip() {
    let mut cpg = CpgLayer::new();
    let source = "class C:\n    def a(self):\n        self.b(42)\n\n    def b(self, x):\n        return x\n";
    build_cpg_for_source(&mut cpg, "/test/t21.py", source);

    let symbol_index = SymbolIndex::new();
    CallGraphBuilder::resolve_all(&mut cpg, &symbol_index);

    // Verify the call resolves
    let a_idx = find_function(&cpg, "/test/t21.py", "a").unwrap();
    let b_idx = find_function(&cpg, "/test/t21.py", "b").unwrap();
    assert!(has_calls_edge(&cpg, a_idx, b_idx), "a should call b");

    // Verify arg flow: arg 0 (42) maps to param after self → position 0 in real_params
    let arg_edges = get_dataflow_arg_edges(&cpg);
    assert!(!arg_edges.is_empty(), "Should have DataFlowArgument edges");
    // Position 0 should be present (42 maps to x, which is position 0 after self skip)
    let positions: HashSet<usize> = arg_edges.iter().map(|(_, _, pos)| *pos).collect();
    assert!(positions.contains(&0), "Arg 42 should map to position 0 (after self skip)");
}

// ============================================================
// Duplicate Edge Regression Tests (22-23)
// ============================================================

// Test 22: Same-file resolve_file produces no duplicate edges, even after re-resolve
#[test]
fn test_resolve_file_no_duplicate_edges_same_file() {
    let mut cpg = CpgLayer::new();
    // Function F is called by 3 different functions in the same file
    let source = "\
def target():
    pass

def caller_a():
    target()

def caller_b():
    target()

def caller_c():
    target()
";
    let path = PathBuf::from("/test/dup1.py");
    build_cpg_for_source(&mut cpg, "/test/dup1.py", source);

    let symbol_index = SymbolIndex::new();
    CallGraphBuilder::resolve_file(&mut cpg, &path, &symbol_index);

    let target_idx = find_function(&cpg, "/test/dup1.py", "target").unwrap();
    let caller_a_idx = find_function(&cpg, "/test/dup1.py", "caller_a").unwrap();
    let caller_b_idx = find_function(&cpg, "/test/dup1.py", "caller_b").unwrap();
    let caller_c_idx = find_function(&cpg, "/test/dup1.py", "caller_c").unwrap();

    // Each caller should have exactly 1 Calls edge to target
    assert_eq!(count_calls_edges(&cpg, caller_a_idx, target_idx), 1, "caller_a→target should have 1 Calls edge");
    assert_eq!(count_calls_edges(&cpg, caller_b_idx, target_idx), 1, "caller_b→target should have 1 Calls edge");
    assert_eq!(count_calls_edges(&cpg, caller_c_idx, target_idx), 1, "caller_c→target should have 1 Calls edge");

    // target should have exactly 3 unique callers
    let callers = cpg.get_callers(target_idx);
    assert_eq!(callers.len(), 3, "target should have exactly 3 callers, got {}", callers.len());

    // Each caller should see exactly 1 callee
    let callees_a = cpg.get_callees(caller_a_idx);
    assert_eq!(callees_a.len(), 1, "caller_a should have exactly 1 callee, got {}", callees_a.len());

    // Re-resolve and verify no accumulation
    CallGraphBuilder::resolve_file(&mut cpg, &path, &symbol_index);

    let callers_after = cpg.get_callers(target_idx);
    assert_eq!(callers_after.len(), 3, "After re-resolve, target should still have 3 callers, got {}", callers_after.len());

    assert_eq!(count_calls_edges(&cpg, caller_a_idx, target_idx), 1, "After re-resolve: caller_a→target should still have 1 Calls edge");
    assert_eq!(count_calls_edges(&cpg, caller_b_idx, target_idx), 1, "After re-resolve: caller_b→target should still have 1 Calls edge");
    assert_eq!(count_calls_edges(&cpg, caller_c_idx, target_idx), 1, "After re-resolve: caller_c→target should still have 1 Calls edge");
}

// Test 23: Cross-file resolve_file produces no duplicate edges
#[test]
fn test_resolve_file_no_duplicate_edges_cross_file() {
    let mut cpg = CpgLayer::new();
    let source_a = "\
def helper():
    pass
";
    let source_b = "\
def use1():
    helper()

def use2():
    helper()
";
    let path_a = PathBuf::from("/test/a.py");
    let path_b = PathBuf::from("/test/b.py");

    build_cpg_for_source(&mut cpg, "/test/a.py", source_a);
    build_cpg_for_source(&mut cpg, "/test/b.py", source_b);

    // Symbol index tells us helper is defined in a.py
    let mut symbol_index = SymbolIndex::new();
    symbol_index.definitions.insert(
        "helper".to_string(),
        vec![path_a.clone()],
    );

    // Resolve file A then file B
    CallGraphBuilder::resolve_file(&mut cpg, &path_a, &symbol_index);
    CallGraphBuilder::resolve_file(&mut cpg, &path_b, &symbol_index);

    let helper_idx = find_function(&cpg, "/test/a.py", "helper").unwrap();
    let use1_idx = find_function(&cpg, "/test/b.py", "use1").unwrap();
    let use2_idx = find_function(&cpg, "/test/b.py", "use2").unwrap();

    assert_eq!(count_calls_edges(&cpg, use1_idx, helper_idx), 1, "use1→helper should have 1 Calls edge");
    assert_eq!(count_calls_edges(&cpg, use2_idx, helper_idx), 1, "use2→helper should have 1 Calls edge");

    let callers = cpg.get_callers(helper_idx);
    assert_eq!(callers.len(), 2, "helper should have exactly 2 callers, got {}", callers.len());

    // Re-resolve file A (simulating MCP's resolve_cpg_for_file pattern)
    CallGraphBuilder::resolve_file(&mut cpg, &path_a, &symbol_index);

    let callers_after = cpg.get_callers(helper_idx);
    assert_eq!(callers_after.len(), 2, "After re-resolve of A, helper should still have 2 callers, got {}", callers_after.len());

    assert_eq!(count_calls_edges(&cpg, use1_idx, helper_idx), 1, "After re-resolve: use1→helper should still have 1 Calls edge");
    assert_eq!(count_calls_edges(&cpg, use2_idx, helper_idx), 1, "After re-resolve: use2→helper should still have 1 Calls edge");
}

// ---------------------------------------------------------------------------
// Import-aware resolution tests
// ---------------------------------------------------------------------------

// Test: module-qualified resolution (`import utils` → `utils.process()`)
#[test]
fn test_module_qualified_call_resolution() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "utils.py", r#"def process(x):
    return x + 1
"#);
    create_test_file(&root_path, "main.py", r#"import utils

def run():
    utils.process(42)
"#);

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    graph.enable_cpg();
    let paths = vec![root_path.join("utils.py"), root_path.join("main.py")];
    graph.build_complete(&paths, &root_path);

    let cpg = graph.cpg.as_ref().unwrap();

    let run_idx = find_function(cpg, &root_path.join("main.py").to_string_lossy(), "run").unwrap();
    let process_idx = find_function(cpg, &root_path.join("utils.py").to_string_lossy(), "process").unwrap();

    assert!(has_calls_edge(cpg, run_idx, process_idx),
        "run() should call utils.process() via module-qualified resolution");
    assert!(has_called_by_edge(cpg, process_idx, run_idx),
        "process() should be called by run()");
}

// Test: from-import class-qualified resolution (`from models import User` → `User.save()`)
#[test]
fn test_class_qualified_call_resolution() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "models.py", r#"class User:
    def save(self):
        pass
"#);
    create_test_file(&root_path, "views.py", r#"from models import User

def create_user():
    User.save(instance)
"#);

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    graph.enable_cpg();
    let paths = vec![root_path.join("models.py"), root_path.join("views.py")];
    graph.build_complete(&paths, &root_path);

    let cpg = graph.cpg.as_ref().unwrap();

    let create_user_idx = find_function(cpg, &root_path.join("views.py").to_string_lossy(), "create_user").unwrap();
    let save_idx = find_function(cpg, &root_path.join("models.py").to_string_lossy(), "save").unwrap();

    assert!(has_calls_edge(cpg, create_user_idx, save_idx),
        "create_user() should call User.save() via class-qualified resolution");
}

// Test: aliased import resolution (`import utils as u` → `u.process()`)
#[test]
fn test_aliased_module_import_resolution() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "utils.py", r#"def process(x):
    return x + 1
"#);
    create_test_file(&root_path, "main.py", r#"import utils as u

def run():
    u.process(42)
"#);

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    graph.enable_cpg();
    let paths = vec![root_path.join("utils.py"), root_path.join("main.py")];
    graph.build_complete(&paths, &root_path);

    let cpg = graph.cpg.as_ref().unwrap();

    let run_idx = find_function(cpg, &root_path.join("main.py").to_string_lossy(), "run").unwrap();
    let process_idx = find_function(cpg, &root_path.join("utils.py").to_string_lossy(), "process").unwrap();

    assert!(has_calls_edge(cpg, run_idx, process_idx),
        "run() should call u.process() (aliased import) via module-qualified resolution");
}

// Test: unresolved when receiver doesn't match any import
#[test]
fn test_unresolved_receiver_no_import() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "main.py", r#"def run():
    unknown.process(42)
"#);

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    graph.enable_cpg();
    let paths = vec![root_path.join("main.py")];
    graph.build_complete(&paths, &root_path);

    let cpg = graph.cpg.as_ref().unwrap();

    // No Calls edges should exist since `unknown` isn't imported
    let calls_count = count_edge_kind(cpg, |e| *e == CpgEdge::Calls);
    assert_eq!(calls_count, 0, "Should have no Calls edges for unresolved receiver");
}

// Test: from-import with alias (`from models import User as U` → `U.save()`)
#[test]
fn test_aliased_from_import_class_resolution() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "models.py", r#"class User:
    def save(self):
        pass
"#);
    create_test_file(&root_path, "views.py", r#"from models import User as U

def create_user():
    U.save(instance)
"#);

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    graph.enable_cpg();
    let paths = vec![root_path.join("models.py"), root_path.join("views.py")];
    graph.build_complete(&paths, &root_path);

    let cpg = graph.cpg.as_ref().unwrap();

    let create_user_idx = find_function(cpg, &root_path.join("views.py").to_string_lossy(), "create_user").unwrap();
    let save_idx = find_function(cpg, &root_path.join("models.py").to_string_lossy(), "save").unwrap();

    assert!(has_calls_edge(cpg, create_user_idx, save_idx),
        "create_user() should call U.save() (aliased from-import) via class-qualified resolution");
}

// ===========================================================================
// build_cpg_for_file tests — build WITHOUT resolve, then resolve separately
// ===========================================================================

/// Build two files with build_cpg_for_file (no resolve), verify zero call edges,
/// then resolve_file on the caller, verify correct Calls/CalledBy edges appear.
#[test]
fn test_build_cpg_for_file_then_resolve() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "utils.py", r#"def helper():
    pass
"#);
    create_test_file(&root_path, "main.py", r#"from utils import helper

def run():
    helper()
"#);

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    let paths = vec![root_path.join("utils.py"), root_path.join("main.py")];
    graph.build_complete(&paths, &root_path);

    // Build CPG for both files WITHOUT resolution
    assert!(graph.build_cpg_for_file(&root_path.join("utils.py")));
    assert!(graph.build_cpg_for_file(&root_path.join("main.py")));

    let cpg = graph.cpg.as_ref().unwrap();

    // No Calls/CalledBy edges should exist yet
    let calls_count = count_edge_kind(cpg, |e| *e == CpgEdge::Calls);
    let called_by_count = count_edge_kind(cpg, |e| *e == CpgEdge::CalledBy);
    assert_eq!(calls_count, 0, "No Calls edges before resolve");
    assert_eq!(called_by_count, 0, "No CalledBy edges before resolve");

    // Now resolve the caller file
    let cpg = graph.cpg.as_mut().unwrap();
    CallGraphBuilder::resolve_file(cpg, &root_path.join("main.py"), &graph.symbol_index);

    let run_idx = find_function(cpg, &root_path.join("main.py").to_string_lossy(), "run").unwrap();
    let helper_idx = find_function(cpg, &root_path.join("utils.py").to_string_lossy(), "helper").unwrap();

    assert!(has_calls_edge(cpg, run_idx, helper_idx),
        "run() should call helper() after resolve_file");
    assert!(has_called_by_edge(cpg, helper_idx, run_idx),
        "helper() should have CalledBy edge from run() after resolve_file");
}

/// Build a file with same-file calls using build_cpg_for_file.
/// Confirm no Calls/CalledBy edges exist (resolution was skipped).
#[test]
fn test_build_cpg_for_file_no_interprocedural_edges() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "app.py", r#"def greet():
    pass

def main():
    greet()
"#);

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    let paths = vec![root_path.join("app.py")];
    graph.build_complete(&paths, &root_path);

    assert!(graph.build_cpg_for_file(&root_path.join("app.py")));

    let cpg = graph.cpg.as_ref().unwrap();
    let calls_count = count_edge_kind(cpg, |e| *e == CpgEdge::Calls);
    let called_by_count = count_edge_kind(cpg, |e| *e == CpgEdge::CalledBy);
    assert_eq!(calls_count, 0, "No Calls edges without resolve");
    assert_eq!(called_by_count, 0, "No CalledBy edges without resolve");
}

/// Call build_cpg_for_file twice on the same file — node count should be unchanged.
#[test]
fn test_build_cpg_for_file_idempotent() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "lib.py", r#"def foo():
    pass

def bar():
    pass
"#);

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    let paths = vec![root_path.join("lib.py")];
    graph.build_complete(&paths, &root_path);

    assert!(graph.build_cpg_for_file(&root_path.join("lib.py")));
    let count_after_first = graph.cpg.as_ref().unwrap().graph.node_count();

    assert!(graph.build_cpg_for_file(&root_path.join("lib.py")));
    let count_after_second = graph.cpg.as_ref().unwrap().graph.node_count();

    assert_eq!(count_after_first, count_after_second,
        "Second build_cpg_for_file call should be a no-op");
}

// ============================================================
// Duplicate-Name Disambiguation Tests (24-27)
// ============================================================

fn find_all_functions(cpg: &CpgLayer, path: &str, name: &str) -> Vec<(NodeIndex, CpgNodeKind)> {
    let path = PathBuf::from(path);
    cpg.file_to_nodes.get(&path)
        .map(|indices| {
            indices.iter().filter_map(|idx| {
                cpg.graph.node_weight(*idx).and_then(|n| {
                    if matches!(n.kind, CpgNodeKind::Function | CpgNodeKind::Method)
                        && n.name == name
                    {
                        Some((*idx, n.kind.clone()))
                    } else {
                        None
                    }
                })
            }).collect()
        })
        .unwrap_or_default()
}

// Test 24: Module-level function + method with same name — internal call resolves
// Reproduces Airflow's dag.py pattern: DagModel.get_last_dagrun() calls get_last_dagrun()
#[test]
fn test_duplicate_name_internal_call_resolves() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "dag.py", r#"
def get_last_dagrun(dag_id, session=None):
    """Module-level function."""
    pass

class DagModel:
    def get_last_dagrun(self, session=None):
        """Method that delegates to module-level function."""
        return get_last_dagrun(self.dag_id, session)
"#);

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    let paths = vec![root_path.join("dag.py")];
    graph.build_complete(&paths, &root_path);

    assert!(graph.ensure_cpg_for_file(&root_path.join("dag.py")));

    let cpg = graph.cpg.as_ref().unwrap();
    let dag_path_str = root_path.join("dag.py").to_string_lossy().to_string();

    // Should have 2 nodes named get_last_dagrun: one Function, one Method
    let all_matches = find_all_functions(cpg, &dag_path_str, "get_last_dagrun");
    assert_eq!(all_matches.len(), 2, "Should have 2 nodes named get_last_dagrun");

    let has_function = all_matches.iter().any(|(_, k)| *k == CpgNodeKind::Function);
    let has_method = all_matches.iter().any(|(_, k)| *k == CpgNodeKind::Method);
    assert!(has_function, "Should have a Function node");
    assert!(has_method, "Should have a Method node");

    // The internal call (method → module-level function) should be resolved
    let calls_count = count_edge_kind(cpg, |e| *e == CpgEdge::Calls);
    assert!(calls_count > 0, "Internal call from method to module-level function should resolve");

    // Specifically: the Method should call the Function
    let method_idx = all_matches.iter().find(|(_, k)| *k == CpgNodeKind::Method).unwrap().0;
    let function_idx = all_matches.iter().find(|(_, k)| *k == CpgNodeKind::Function).unwrap().0;
    assert!(has_calls_edge(cpg, method_idx, function_idx),
        "Method should have Calls edge to module-level function");
    assert!(has_called_by_edge(cpg, function_idx, method_idx),
        "Module-level function should have CalledBy edge from method");
}

// Test 25: Cross-file instance method call resolves to Method, not Function
#[test]
fn test_cross_file_instance_method_resolves() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "dag.py", r#"
def get_last_dagrun(dag_id, session=None):
    pass

class DagModel:
    def get_last_dagrun(self, session=None):
        return get_last_dagrun(self.dag_id, session)
"#);

    create_test_file(&root_path, "assets.py", r#"from dag import DagModel

def get_asset_info(session):
    dag_model = DagModel()
    latest_run = dag_model.get_last_dagrun(session=session)
    return latest_run
"#);

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    let paths = vec![root_path.join("dag.py"), root_path.join("assets.py")];
    graph.build_complete(&paths, &root_path);

    // Build CPG for both files, then resolve
    assert!(graph.ensure_cpg_for_file(&root_path.join("dag.py")));
    assert!(graph.build_cpg_for_file(&root_path.join("assets.py")));

    let cpg = graph.cpg.as_mut().unwrap();
    CallGraphBuilder::resolve_file(cpg, &root_path.join("dag.py"), &graph.symbol_index);

    let dag_path_str = root_path.join("dag.py").to_string_lossy().to_string();
    let assets_path_str = root_path.join("assets.py").to_string_lossy().to_string();

    let all_dag_matches = find_all_functions(cpg, &dag_path_str, "get_last_dagrun");
    let get_asset_info = find_function(cpg, &assets_path_str, "get_asset_info");

    assert!(get_asset_info.is_some(), "get_asset_info should exist in CPG");
    let caller_idx = get_asset_info.unwrap();

    // The cross-file call should resolve to the Method (since it has a receiver)
    let method_idx = all_dag_matches.iter()
        .find(|(_, k)| *k == CpgNodeKind::Method)
        .map(|(idx, _)| *idx);

    assert!(method_idx.is_some(), "Should have a Method node for get_last_dagrun");
    let method_idx = method_idx.unwrap();

    assert!(has_calls_edge(cpg, caller_idx, method_idx),
        "get_asset_info should have Calls edge to DagModel.get_last_dagrun method");
    assert!(has_called_by_edge(cpg, method_idx, caller_idx),
        "DagModel.get_last_dagrun method should have CalledBy edge from get_asset_info");
}

// Test 26: Unique name still resolves correctly (regression guard)
#[test]
fn test_unique_name_still_resolves() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "utils.py", r#"
def compute_result(data):
    pass
"#);

    create_test_file(&root_path, "main.py", r#"from utils import compute_result

def run():
    compute_result([1, 2, 3])
"#);

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    let paths = vec![root_path.join("utils.py"), root_path.join("main.py")];
    graph.build_complete(&paths, &root_path);

    assert!(graph.ensure_cpg_for_file(&root_path.join("utils.py")));
    assert!(graph.build_cpg_for_file(&root_path.join("main.py")));
    let cpg = graph.cpg.as_mut().unwrap();
    CallGraphBuilder::resolve_file(cpg, &root_path.join("utils.py"), &graph.symbol_index);

    let calls_count = count_edge_kind(cpg, |e| *e == CpgEdge::Calls);
    let called_by_count = count_edge_kind(cpg, |e| *e == CpgEdge::CalledBy);

    assert!(calls_count > 0, "Unique name should resolve — Calls edges expected");
    assert!(called_by_count > 0, "Unique name should resolve — CalledBy edges expected");
}

// Test 27: find_all_func_indices returns both Function and Method nodes
#[test]
fn test_find_all_func_indices_returns_both() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "dag.py", r#"
def get_last_dagrun(dag_id, session=None):
    pass

class DagModel:
    def get_last_dagrun(self, session=None):
        pass
"#);

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    let paths = vec![root_path.join("dag.py")];
    graph.build_complete(&paths, &root_path);

    assert!(graph.ensure_cpg_for_file(&root_path.join("dag.py")));

    let cpg = graph.cpg.as_ref().unwrap();
    let dag_path_str = root_path.join("dag.py").to_string_lossy().to_string();

    let all_matches = find_all_functions(cpg, &dag_path_str, "get_last_dagrun");
    assert_eq!(all_matches.len(), 2, "Should find both Function and Method named get_last_dagrun");

    let kinds: Vec<&CpgNodeKind> = all_matches.iter().map(|(_, k)| k).collect();
    assert!(kinds.contains(&&CpgNodeKind::Function), "Should include a Function node");
    assert!(kinds.contains(&&CpgNodeKind::Method), "Should include a Method node");
}
