use semantic_engine::cpg::{CpgEdge, CpgLayer, CpgNodeKind, StatementKind};
use semantic_engine::graph::RepoGraph;
use semantic_engine::parser::SupportedLanguage;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;
use tree_sitter::Parser as TreeSitterParser;

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
        cpg.graph.node_weight(**idx)
            .map(|n| {
                matches!(n.kind, CpgNodeKind::Function | CpgNodeKind::Method)
                    && n.name == name
            })
            .unwrap_or(false)
    }).copied()
}

fn get_statements(cpg: &CpgLayer, func_idx: NodeIndex) -> Vec<(NodeIndex, String, Option<StatementKind>)> {
    let mut result = Vec::new();
    for idx in cpg.graph.node_indices() {
        if let Some(node) = cpg.graph.node_weight(idx) {
            if node.function_idx == Some(func_idx) && node.kind == CpgNodeKind::Statement {
                result.push((idx, node.name.clone(), node.statement_kind.clone()));
            }
        }
    }
    result.sort_by_key(|(idx, _, _)| cpg.graph.node_weight(*idx).map(|n| n.start_line).unwrap_or(0));
    result
}

/// Get all DataFlowReach edges for a function as (def_idx, use_idx, var_name) tuples.
fn get_dataflow_edges(cpg: &CpgLayer, func_idx: NodeIndex) -> Vec<(NodeIndex, NodeIndex, String)> {
    cpg.get_dataflow_edges_for_function(func_idx)
        .iter()
        .map(|(s, t, v)| (*s, *t, v.to_string()))
        .collect()
}

/// Get the defs for a specific node.
fn get_defs(cpg: &CpgLayer, idx: NodeIndex) -> Vec<String> {
    cpg.stmt_defs.get(&idx).cloned().unwrap_or_default()
}

/// Get the uses for a specific node.
fn get_uses(cpg: &CpgLayer, idx: NodeIndex) -> Vec<String> {
    cpg.stmt_uses.get(&idx).cloned().unwrap_or_default()
}

fn create_test_file(root: &std::path::Path, path: &str, content: &str) {
    let file_path = root.join(path);
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(file_path, content).unwrap();
}

// ============================================================
// Def/Use Extraction Tests
// ============================================================

// Test 1: Simple assignment: x = 1 → defs: {x}, uses: {}
#[test]
fn test_simple_assignment_defs_uses() {
    let mut cpg = CpgLayer::new();
    let source = "def f():\n    x = 1\n";
    build_cpg_for_source(&mut cpg, "/test/t1.py", source);

    let func_idx = find_function(&cpg, "/test/t1.py", "f").unwrap();
    let stmts = get_statements(&cpg, func_idx);
    assert_eq!(stmts.len(), 1);

    let defs = get_defs(&cpg, stmts[0].0);
    let uses = get_uses(&cpg, stmts[0].0);
    assert_eq!(defs, vec!["x"]);
    assert!(uses.is_empty());
}

// Test 2: Assignment with expression: x = y + z → defs: {x}, uses: {y, z}
#[test]
fn test_assignment_with_expression() {
    let mut cpg = CpgLayer::new();
    let source = "def f(y, z):\n    x = y + z\n";
    build_cpg_for_source(&mut cpg, "/test/t2.py", source);

    let func_idx = find_function(&cpg, "/test/t2.py", "f").unwrap();
    let stmts = get_statements(&cpg, func_idx);
    assert_eq!(stmts.len(), 1);

    let defs = get_defs(&cpg, stmts[0].0);
    let uses = get_uses(&cpg, stmts[0].0);
    assert_eq!(defs, vec!["x"]);
    assert!(uses.contains(&"y".to_string()));
    assert!(uses.contains(&"z".to_string()));
}

// Test 3: Augmented assignment: x += y → defs: {x}, uses: {x, y}
#[test]
fn test_augmented_assignment() {
    let mut cpg = CpgLayer::new();
    let source = "def f(x, y):\n    x += y\n";
    build_cpg_for_source(&mut cpg, "/test/t3.py", source);

    let func_idx = find_function(&cpg, "/test/t3.py", "f").unwrap();
    let stmts = get_statements(&cpg, func_idx);
    assert_eq!(stmts.len(), 1);

    let defs = get_defs(&cpg, stmts[0].0);
    let uses = get_uses(&cpg, stmts[0].0);
    assert_eq!(defs, vec!["x"]);
    assert!(uses.contains(&"x".to_string()));
    assert!(uses.contains(&"y".to_string()));
}

// Test 4: Tuple unpacking: a, b = f() → defs: {a, b}, uses: {f}
#[test]
fn test_tuple_unpacking() {
    let mut cpg = CpgLayer::new();
    let source = "def g():\n    a, b = f()\n";
    build_cpg_for_source(&mut cpg, "/test/t4.py", source);

    let func_idx = find_function(&cpg, "/test/t4.py", "g").unwrap();
    let stmts = get_statements(&cpg, func_idx);
    assert_eq!(stmts.len(), 1);

    let defs = get_defs(&cpg, stmts[0].0);
    let uses = get_uses(&cpg, stmts[0].0);
    assert!(defs.contains(&"a".to_string()));
    assert!(defs.contains(&"b".to_string()));
    assert!(uses.contains(&"f".to_string()));
}

// Test 5: For loop variable: for x in items → defs: {x}, uses: {items}
#[test]
fn test_for_loop_variable() {
    let mut cpg = CpgLayer::new();
    let source = "def f(items):\n    for x in items:\n        pass\n";
    build_cpg_for_source(&mut cpg, "/test/t5.py", source);

    let func_idx = find_function(&cpg, "/test/t5.py", "f").unwrap();
    let stmts = get_statements(&cpg, func_idx);

    // Find the for statement
    let for_stmt = stmts.iter().find(|(_, _, sk)| {
        sk.as_ref().map(|k| *k == StatementKind::For).unwrap_or(false)
    }).unwrap();

    let defs = get_defs(&cpg, for_stmt.0);
    let uses = get_uses(&cpg, for_stmt.0);
    assert!(defs.contains(&"x".to_string()));
    assert!(uses.contains(&"items".to_string()));
}

// Test 6: With statement: with open(f) as h → defs: {h}, uses: {open, f}
#[test]
fn test_with_statement_defs_uses() {
    let mut cpg = CpgLayer::new();
    let source = "def f(fname):\n    with open(fname) as h:\n        pass\n";
    build_cpg_for_source(&mut cpg, "/test/t6.py", source);

    let func_idx = find_function(&cpg, "/test/t6.py", "f").unwrap();
    let stmts = get_statements(&cpg, func_idx);

    let with_stmt = stmts.iter().find(|(_, _, sk)| {
        sk.as_ref().map(|k| *k == StatementKind::With).unwrap_or(false)
    }).unwrap();

    let defs = get_defs(&cpg, with_stmt.0);
    let uses = get_uses(&cpg, with_stmt.0);
    assert!(defs.contains(&"h".to_string()), "with should def h, got defs: {:?}", defs);
    assert!(uses.contains(&"open".to_string()), "with should use open, got uses: {:?}", uses);
    assert!(uses.contains(&"fname".to_string()), "with should use fname, got uses: {:?}", uses);
}

// Test 7: Function parameters → CfgEntry defs
#[test]
fn test_function_parameters_at_entry() {
    let mut cpg = CpgLayer::new();
    let source = "def f(a, b):\n    return a + b\n";
    build_cpg_for_source(&mut cpg, "/test/t7.py", source);

    let func_idx = find_function(&cpg, "/test/t7.py", "f").unwrap();
    let entry_idx = cpg.function_to_entry[&func_idx];

    let defs = get_defs(&cpg, entry_idx);
    assert!(defs.contains(&"a".to_string()));
    assert!(defs.contains(&"b".to_string()));
}

// Test 8: Expression statement (call): process(x) → defs: {}, uses: {process, x}
#[test]
fn test_expression_statement_call() {
    let mut cpg = CpgLayer::new();
    let source = "def f(x):\n    process(x)\n";
    build_cpg_for_source(&mut cpg, "/test/t8.py", source);

    let func_idx = find_function(&cpg, "/test/t8.py", "f").unwrap();
    let stmts = get_statements(&cpg, func_idx);
    assert_eq!(stmts.len(), 1);

    let defs = get_defs(&cpg, stmts[0].0);
    let uses = get_uses(&cpg, stmts[0].0);
    assert!(defs.is_empty());
    assert!(uses.contains(&"process".to_string()));
    assert!(uses.contains(&"x".to_string()));
}

// Test 9: Return statement: return x + 1 → defs: {}, uses: {x}
#[test]
fn test_return_statement_uses() {
    let mut cpg = CpgLayer::new();
    let source = "def f(x):\n    return x + 1\n";
    build_cpg_for_source(&mut cpg, "/test/t9.py", source);

    let func_idx = find_function(&cpg, "/test/t9.py", "f").unwrap();
    let stmts = get_statements(&cpg, func_idx);
    assert_eq!(stmts.len(), 1);

    let defs = get_defs(&cpg, stmts[0].0);
    let uses = get_uses(&cpg, stmts[0].0);
    assert!(defs.is_empty());
    assert!(uses.contains(&"x".to_string()));
}

// Test 10: Self filtering: self.x = y → defs: {}, uses: {y}
#[test]
fn test_self_filtering() {
    let mut cpg = CpgLayer::new();
    let source = "class C:\n    def m(self, y):\n        self.x = y\n";
    build_cpg_for_source(&mut cpg, "/test/t10.py", source);

    let func_idx = find_function(&cpg, "/test/t10.py", "m").unwrap();
    let stmts = get_statements(&cpg, func_idx);
    assert_eq!(stmts.len(), 1);

    let defs = get_defs(&cpg, stmts[0].0);
    let uses = get_uses(&cpg, stmts[0].0);
    // self.x is an attribute, not a local variable def
    assert!(defs.is_empty(), "self.x should not be a local def, got: {:?}", defs);
    assert!(uses.contains(&"y".to_string()));
    // 'self' should be filtered out
    assert!(!uses.contains(&"self".to_string()));
}

// ============================================================
// Reaching Definitions Tests
// ============================================================

// Test 11: Linear flow: x = 1; y = x → DataFlowReach("x") from x=1 to y=x
#[test]
fn test_linear_flow() {
    let mut cpg = CpgLayer::new();
    let source = "def f():\n    x = 1\n    y = x\n";
    build_cpg_for_source(&mut cpg, "/test/t11.py", source);

    let func_idx = find_function(&cpg, "/test/t11.py", "f").unwrap();
    let edges = get_dataflow_edges(&cpg, func_idx);

    // Should have DataFlowReach("x") from x=1 to y=x
    let x_edges: Vec<_> = edges.iter().filter(|(_, _, v)| v == "x").collect();
    assert!(!x_edges.is_empty(), "Should have DataFlowReach for x, got: {:?}", edges);

    let stmts = get_statements(&cpg, func_idx);
    let x_def = stmts[0].0; // x = 1
    let y_use = stmts[1].0; // y = x

    assert!(
        x_edges.iter().any(|(s, t, _)| *s == x_def && *t == y_use),
        "DataFlowReach(x) should go from x=1 to y=x"
    );
}

// Test 12: Kill: x = 1; x = 2; y = x → only x=2 reaches y=x
#[test]
fn test_kill() {
    let mut cpg = CpgLayer::new();
    let source = "def f():\n    x = 1\n    x = 2\n    y = x\n";
    build_cpg_for_source(&mut cpg, "/test/t12.py", source);

    let func_idx = find_function(&cpg, "/test/t12.py", "f").unwrap();
    let stmts = get_statements(&cpg, func_idx);
    let x1 = stmts[0].0; // x = 1
    let x2 = stmts[1].0; // x = 2
    let y = stmts[2].0;  // y = x

    let edges = get_dataflow_edges(&cpg, func_idx);
    let x_to_y: Vec<_> = edges.iter().filter(|(_, t, v)| *t == y && v == "x").collect();

    // Only x=2 should reach y=x (x=1 is killed by x=2)
    assert!(
        x_to_y.iter().any(|(s, _, _)| *s == x2),
        "x=2 should reach y=x"
    );
    assert!(
        !x_to_y.iter().any(|(s, _, _)| *s == x1),
        "x=1 should NOT reach y=x (killed by x=2)"
    );
}

// Test 13: Branch convergence: if c: x = 1 else: x = 2; y = x → two defs reach y
#[test]
fn test_branch_convergence() {
    let mut cpg = CpgLayer::new();
    let source = "def f(c):\n    if c:\n        x = 1\n    else:\n        x = 2\n    y = x\n";
    build_cpg_for_source(&mut cpg, "/test/t13.py", source);

    let func_idx = find_function(&cpg, "/test/t13.py", "f").unwrap();
    let stmts = get_statements(&cpg, func_idx);

    // Find y = x statement
    let y_stmt = stmts.iter().find(|(_, name, _)| name.contains("y = x") || name.contains("y =")).unwrap();

    let edges = get_dataflow_edges(&cpg, func_idx);
    let x_to_y: Vec<_> = edges.iter().filter(|(_, t, v)| *t == y_stmt.0 && v == "x").collect();

    // Both x=1 and x=2 should reach y=x
    assert_eq!(x_to_y.len(), 2, "Both branches should reach y=x, got: {:?}", x_to_y);
}

// Test 14: Branch without else: x = 0; if c: x = 1; y = x → both x=0 and x=1 reach y=x
#[test]
fn test_branch_no_else() {
    let mut cpg = CpgLayer::new();
    let source = "def f(c):\n    x = 0\n    if c:\n        x = 1\n    y = x\n";
    build_cpg_for_source(&mut cpg, "/test/t14.py", source);

    let func_idx = find_function(&cpg, "/test/t14.py", "f").unwrap();
    let stmts = get_statements(&cpg, func_idx);

    // Find y = x
    let y_stmt = stmts.iter().find(|(_, name, _)| name.contains("y =")).unwrap();

    let edges = get_dataflow_edges(&cpg, func_idx);
    let x_to_y: Vec<_> = edges.iter().filter(|(_, t, v)| *t == y_stmt.0 && v == "x").collect();

    // Both x=0 and x=1 should reach y=x
    assert_eq!(x_to_y.len(), 2, "Both x=0 and x=1 should reach y=x, got: {:?}", x_to_y);
}

// Test 15: Loop: x = 0; while c: x = x + 1 → x=0 and x=x+1 both reach the while body
#[test]
fn test_loop_reaching_defs() {
    let mut cpg = CpgLayer::new();
    let source = "def f(c):\n    x = 0\n    while c:\n        x = x + 1\n";
    build_cpg_for_source(&mut cpg, "/test/t15.py", source);

    let func_idx = find_function(&cpg, "/test/t15.py", "f").unwrap();
    let stmts = get_statements(&cpg, func_idx);

    // Find x = x + 1 (the augmented or assignment in loop body)
    let loop_body = stmts.iter().find(|(_, name, _)| name.contains("x = x")).unwrap();

    let edges = get_dataflow_edges(&cpg, func_idx);
    let x_to_body: Vec<_> = edges.iter().filter(|(_, t, v)| *t == loop_body.0 && v == "x").collect();

    // Both x=0 and x=x+1 should reach the loop body usage of x
    assert!(x_to_body.len() >= 2, "Both x=0 and x=x+1 should reach the loop body, got {} edges", x_to_body.len());
}

// Test 16: Parameters: def f(a, b): return a + b → CfgEntry defs reach return
#[test]
fn test_parameters_reach_return() {
    let mut cpg = CpgLayer::new();
    let source = "def f(a, b):\n    return a + b\n";
    build_cpg_for_source(&mut cpg, "/test/t16.py", source);

    let func_idx = find_function(&cpg, "/test/t16.py", "f").unwrap();
    let entry_idx = cpg.function_to_entry[&func_idx];

    let stmts = get_statements(&cpg, func_idx);
    let ret_stmt = stmts.iter().find(|(_, _, sk)| {
        sk.as_ref().map(|k| *k == StatementKind::Return).unwrap_or(false)
    }).unwrap();

    let edges = get_dataflow_edges(&cpg, func_idx);

    // Parameter 'a' should reach return via CfgEntry
    let a_edges: Vec<_> = edges.iter().filter(|(s, t, v)| *s == entry_idx && *t == ret_stmt.0 && v == "a").collect();
    assert!(!a_edges.is_empty(), "Parameter 'a' should reach return");

    // Parameter 'b' should reach return via CfgEntry
    let b_edges: Vec<_> = edges.iter().filter(|(s, t, v)| *s == entry_idx && *t == ret_stmt.0 && v == "b").collect();
    assert!(!b_edges.is_empty(), "Parameter 'b' should reach return");
}

// Test 17: No reaching def for undefined var → no DataFlowReach edge for that var
#[test]
fn test_no_reaching_def_for_undefined() {
    let mut cpg = CpgLayer::new();
    // 'y' is used but never defined in this function
    let source = "def f():\n    x = y\n";
    build_cpg_for_source(&mut cpg, "/test/t17.py", source);

    let func_idx = find_function(&cpg, "/test/t17.py", "f").unwrap();
    let edges = get_dataflow_edges(&cpg, func_idx);

    // No DataFlowReach edge for 'y' (never defined)
    let y_edges: Vec<_> = edges.iter().filter(|(_, _, v)| v == "y").collect();
    assert!(y_edges.is_empty(), "Should not have DataFlowReach for undefined var y");
}

// ============================================================
// Integration Tests
// ============================================================

// Test 18: RepoGraph integration — enable_cpg, build_complete, verify DataFlowReach edges
#[test]
fn test_repograph_dataflow_integration() {
    let root = tempdir().unwrap();
    let root_path = root.path().to_path_buf();

    create_test_file(&root_path, "example.py", r#"
def add(a, b):
    result = a + b
    return result
"#);

    let mut graph = RepoGraph::new(&root_path, "python", &[]);
    graph.enable_cpg();
    let paths = vec![root_path.join("example.py")];
    graph.build_complete(&paths, &root_path);

    let cpg = graph.cpg.as_ref().unwrap();

    let add_idx = find_function(cpg, &root_path.join("example.py").to_string_lossy(), "add").unwrap();
    let edges = get_dataflow_edges(cpg, add_idx);

    // Should have dataflow edges for a, b reaching result = a + b
    // And result reaching return result
    assert!(!edges.is_empty(), "Should have DataFlowReach edges");

    let a_edges: Vec<_> = edges.iter().filter(|(_, _, v)| v == "a").collect();
    let b_edges: Vec<_> = edges.iter().filter(|(_, _, v)| v == "b").collect();
    let result_edges: Vec<_> = edges.iter().filter(|(_, _, v)| v == "result").collect();

    assert!(!a_edges.is_empty(), "Should have flow for 'a'");
    assert!(!b_edges.is_empty(), "Should have flow for 'b'");
    assert!(!result_edges.is_empty(), "Should have flow for 'result'");
}

// Test 19: Empty function: def f(): pass → no data flow edges
#[test]
fn test_empty_function_no_dataflow() {
    let mut cpg = CpgLayer::new();
    let source = "def f():\n    pass\n";
    build_cpg_for_source(&mut cpg, "/test/t19.py", source);

    let func_idx = find_function(&cpg, "/test/t19.py", "f").unwrap();
    let edges = get_dataflow_edges(&cpg, func_idx);

    assert!(edges.is_empty(), "Empty function should have no DataFlowReach edges");
}

// Test 20: Incremental update — update_file removes old edges, adds new ones
#[test]
fn test_incremental_update_dataflow() {
    let mut cpg = CpgLayer::new();
    let path = "/test/t20.py";

    // Initial version
    let source1 = "def f():\n    x = 1\n    y = x\n";
    build_cpg_for_source(&mut cpg, path, source1);

    let func_idx = find_function(&cpg, path, "f").unwrap();
    let edges1 = get_dataflow_edges(&cpg, func_idx);
    assert!(!edges1.is_empty(), "Should have DataFlowReach edges in v1");

    // Update to a different version
    let source2 = "def f():\n    a = 1\n    b = a\n";
    let path_buf = PathBuf::from(path);
    let mut parser = TreeSitterParser::new();
    parser
        .set_language(SupportedLanguage::Python.get_parser().unwrap())
        .unwrap();
    let tree2 = parser.parse(source2, None).unwrap();
    cpg.update_file(&path_buf, tree2, source2.to_string(), SupportedLanguage::Python);

    let func_idx2 = find_function(&cpg, path, "f").unwrap();
    let edges2 = get_dataflow_edges(&cpg, func_idx2);

    // Old edges (for x) should be gone, new edges (for a) should exist
    let x_edges: Vec<_> = edges2.iter().filter(|(_, _, v)| v == "x").collect();
    let a_edges: Vec<_> = edges2.iter().filter(|(_, _, v)| v == "a").collect();

    assert!(x_edges.is_empty(), "Old var 'x' edges should be removed after update");
    assert!(!a_edges.is_empty(), "New var 'a' edges should exist after update");
}
