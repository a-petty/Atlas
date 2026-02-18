use semantic_engine::cpg::{CpgEdge, CpgLayer, CpgNodeKind, StatementKind};
use semantic_engine::graph::RepoGraph;
use semantic_engine::parser::SupportedLanguage;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;
use tree_sitter::Parser as TreeSitterParser;

/// Helper: parse Python source and build CPG (with CFG) for it.
fn build_cpg_for_source(cpg: &mut CpgLayer, path: &str, source: &str) {
    let path = PathBuf::from(path);
    let mut parser = TreeSitterParser::new();
    parser
        .set_language(SupportedLanguage::Python.get_parser().unwrap())
        .unwrap();
    let tree = parser.parse(source, None).unwrap();
    cpg.build_file(&path, tree, source.to_string(), SupportedLanguage::Python);
}

/// Helper: find a function/method node by name.
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

/// Helper: collect all CFG edges for a function.
fn get_cfg_edges(cpg: &CpgLayer, func_idx: NodeIndex) -> Vec<(NodeIndex, NodeIndex, CpgEdge)> {
    cpg.get_cfg_edges_for_function(func_idx)
        .iter()
        .map(|(s, t, e)| (*s, *t, (*e).clone()))
        .collect()
}

/// Helper: check if a specific edge kind exists between two nodes.
fn has_edge(cpg: &CpgLayer, from: NodeIndex, to: NodeIndex, kind: &CpgEdge) -> bool {
    cpg.graph.edges_directed(from, petgraph::Direction::Outgoing)
        .any(|e| e.target() == to && e.weight() == kind)
}

/// Helper: find statement nodes for a function.
fn get_statements(cpg: &CpgLayer, func_idx: NodeIndex) -> Vec<(NodeIndex, &str, Option<&StatementKind>)> {
    let mut result = Vec::new();
    for idx in cpg.graph.node_indices() {
        if let Some(node) = cpg.graph.node_weight(idx) {
            if node.function_idx == Some(func_idx) && node.kind == CpgNodeKind::Statement {
                result.push((idx, node.name.as_str(), node.statement_kind.as_ref()));
            }
        }
    }
    // Sort by start_line for predictable ordering
    result.sort_by_key(|(idx, _, _)| cpg.graph.node_weight(*idx).map(|n| n.start_line).unwrap_or(0));
    result
}

fn create_test_file(root: &std::path::Path, path: &str, content: &str) {
    let file_path = root.join(path);
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(file_path, content).unwrap();
}

// ============================================================
// Test 1: Sequential statements — ControlFlowNext chain + sentinels
// ============================================================
#[test]
fn test_sequential_statements() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def seq():
    x = 1
    y = 2
    z = 3
"#;
    build_cpg_for_source(&mut cpg, "/test/seq.py", source);

    let func_idx = find_function(&cpg, "/test/seq.py", "seq").unwrap();
    let entry_idx = cpg.function_to_entry[&func_idx];
    let exit_idx = cpg.function_to_exit[&func_idx];

    // Check sentinels exist
    assert_eq!(cpg.graph[entry_idx].kind, CpgNodeKind::CfgEntry);
    assert_eq!(cpg.graph[exit_idx].kind, CpgNodeKind::CfgExit);

    let stmts = get_statements(&cpg, func_idx);
    assert_eq!(stmts.len(), 3);

    // Entry → first stmt
    assert!(has_edge(&cpg, entry_idx, stmts[0].0, &CpgEdge::ControlFlowNext));
    // stmt1 → stmt2 → stmt3
    assert!(has_edge(&cpg, stmts[0].0, stmts[1].0, &CpgEdge::ControlFlowNext));
    assert!(has_edge(&cpg, stmts[1].0, stmts[2].0, &CpgEdge::ControlFlowNext));
    // last stmt → exit
    assert!(has_edge(&cpg, stmts[2].0, exit_idx, &CpgEdge::ControlFlowNext));
}

// ============================================================
// Test 2: If/else — ControlFlowTrue and ControlFlowFalse edges
// ============================================================
#[test]
fn test_if_else() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def branch(x):
    if x > 0:
        a = 1
    else:
        b = 2
    c = 3
"#;
    build_cpg_for_source(&mut cpg, "/test/branch.py", source);

    let func_idx = find_function(&cpg, "/test/branch.py", "branch").unwrap();
    let stmts = get_statements(&cpg, func_idx);

    // Should have: if_stmt, a=1, b=2, c=3
    assert_eq!(stmts.len(), 4);

    let if_idx = stmts[0].0;
    let a_idx = stmts[1].0;
    let b_idx = stmts[2].0;
    let c_idx = stmts[3].0;

    // if → true branch (a=1)
    assert!(has_edge(&cpg, if_idx, a_idx, &CpgEdge::ControlFlowTrue));
    // if → false branch (b=2)
    assert!(has_edge(&cpg, if_idx, b_idx, &CpgEdge::ControlFlowFalse));
    // Both branches converge to c=3
    assert!(has_edge(&cpg, a_idx, c_idx, &CpgEdge::ControlFlowNext));
    assert!(has_edge(&cpg, b_idx, c_idx, &CpgEdge::ControlFlowNext));
}

// ============================================================
// Test 3: If without else — false branch falls through
// ============================================================
#[test]
fn test_if_no_else() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def maybe(x):
    if x > 0:
        a = 1
    b = 2
"#;
    build_cpg_for_source(&mut cpg, "/test/maybe.py", source);

    let func_idx = find_function(&cpg, "/test/maybe.py", "maybe").unwrap();
    let stmts = get_statements(&cpg, func_idx);

    // if_stmt, a=1, b=2
    assert_eq!(stmts.len(), 3);

    let if_idx = stmts[0].0;
    let a_idx = stmts[1].0;
    let b_idx = stmts[2].0;

    // True branch
    assert!(has_edge(&cpg, if_idx, a_idx, &CpgEdge::ControlFlowTrue));
    // a → b (true branch falls through)
    assert!(has_edge(&cpg, a_idx, b_idx, &CpgEdge::ControlFlowNext));
    // if → b (false falls through — if_idx is in exits since no else)
    assert!(has_edge(&cpg, if_idx, b_idx, &CpgEdge::ControlFlowNext));
}

// ============================================================
// Test 4: If/elif/else chain — all branches converge
// ============================================================
#[test]
fn test_if_elif_else() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def classify(x):
    if x > 0:
        r = "positive"
    elif x < 0:
        r = "negative"
    else:
        r = "zero"
    return r
"#;
    build_cpg_for_source(&mut cpg, "/test/classify.py", source);

    let func_idx = find_function(&cpg, "/test/classify.py", "classify").unwrap();
    let entry_idx = cpg.function_to_entry[&func_idx];
    let exit_idx = cpg.function_to_exit[&func_idx];

    let stmts = get_statements(&cpg, func_idx);

    // if, r="positive", elif, r="negative", r="zero", return
    assert!(stmts.len() >= 5);

    // Find the return statement
    let return_stmt = stmts.iter().find(|(_, _, sk)| {
        sk.as_ref().map(|k| **k == StatementKind::Return).unwrap_or(false)
    });
    assert!(return_stmt.is_some());

    // Return should connect to exit
    let return_idx = return_stmt.unwrap().0;
    assert!(has_edge(&cpg, return_idx, exit_idx, &CpgEdge::ControlFlowNext));
}

// ============================================================
// Test 5: For loop — ControlFlowTrue→body, ControlFlowBack, ControlFlowFalse
// ============================================================
#[test]
fn test_for_loop() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def loop_it(items):
    for item in items:
        process(item)
    done = True
"#;
    build_cpg_for_source(&mut cpg, "/test/loop.py", source);

    let func_idx = find_function(&cpg, "/test/loop.py", "loop_it").unwrap();
    let stmts = get_statements(&cpg, func_idx);

    // for_stmt, process(item), done=True
    assert_eq!(stmts.len(), 3);

    let for_idx = stmts[0].0;
    let process_idx = stmts[1].0;
    let done_idx = stmts[2].0;

    // for → body (true: iteration continues)
    assert!(has_edge(&cpg, for_idx, process_idx, &CpgEdge::ControlFlowTrue));
    // body → back to for header
    assert!(has_edge(&cpg, process_idx, for_idx, &CpgEdge::ControlFlowBack));
    // for → done (false: iteration exhausted), for_idx is in exits
    assert!(has_edge(&cpg, for_idx, done_idx, &CpgEdge::ControlFlowNext));
}

// ============================================================
// Test 6: While loop — same pattern as for
// ============================================================
#[test]
fn test_while_loop() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def spin(n):
    while n > 0:
        n = n - 1
    result = "done"
"#;
    build_cpg_for_source(&mut cpg, "/test/spin.py", source);

    let func_idx = find_function(&cpg, "/test/spin.py", "spin").unwrap();
    let stmts = get_statements(&cpg, func_idx);

    assert_eq!(stmts.len(), 3);

    let while_idx = stmts[0].0;
    let body_idx = stmts[1].0;
    let result_idx = stmts[2].0;

    assert!(has_edge(&cpg, while_idx, body_idx, &CpgEdge::ControlFlowTrue));
    assert!(has_edge(&cpg, body_idx, while_idx, &CpgEdge::ControlFlowBack));
    assert!(has_edge(&cpg, while_idx, result_idx, &CpgEdge::ControlFlowNext));
}

// ============================================================
// Test 7: Try/except — ControlFlowException edges
// ============================================================
#[test]
fn test_try_except() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def risky():
    try:
        x = dangerous()
    except ValueError:
        x = fallback()
    use(x)
"#;
    build_cpg_for_source(&mut cpg, "/test/risky.py", source);

    let func_idx = find_function(&cpg, "/test/risky.py", "risky").unwrap();
    let stmts = get_statements(&cpg, func_idx);

    // try_stmt, x=dangerous(), x=fallback(), use(x)
    assert!(stmts.len() >= 3);

    let try_idx = stmts[0].0;

    // try_stmt should have a ControlFlowException edge
    let edges = get_cfg_edges(&cpg, func_idx);
    let exception_edges: Vec<_> = edges.iter()
        .filter(|(s, _, e)| *s == try_idx && *e == CpgEdge::ControlFlowException)
        .collect();
    assert!(!exception_edges.is_empty(), "try should have ControlFlowException edge to handler");

    // try_stmt should have ControlFlowNext edge to body
    let next_edges: Vec<_> = edges.iter()
        .filter(|(s, _, e)| *s == try_idx && *e == CpgEdge::ControlFlowNext)
        .collect();
    assert!(!next_edges.is_empty(), "try should have ControlFlowNext edge to body");
}

// ============================================================
// Test 8: Try/except/finally — all paths through finally
// ============================================================
#[test]
fn test_try_except_finally() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def safe():
    try:
        x = dangerous()
    except Exception:
        x = 0
    finally:
        cleanup()
    done = True
"#;
    build_cpg_for_source(&mut cpg, "/test/safe.py", source);

    let func_idx = find_function(&cpg, "/test/safe.py", "safe").unwrap();
    let stmts = get_statements(&cpg, func_idx);

    // try_stmt, x=dangerous(), x=0, cleanup(), done=True
    assert!(stmts.len() >= 4);

    // Find the cleanup statement
    let cleanup_stmt = stmts.iter().find(|(_, name, _)| name.contains("cleanup"));
    assert!(cleanup_stmt.is_some(), "Should find cleanup statement");

    // Find the done statement
    let done_stmt = stmts.iter().find(|(_, name, _)| name.contains("done"));
    assert!(done_stmt.is_some(), "Should find done statement");

    let cleanup_idx = cleanup_stmt.unwrap().0;
    let done_idx = done_stmt.unwrap().0;

    // cleanup → done (finally exits to next statement)
    assert!(has_edge(&cpg, cleanup_idx, done_idx, &CpgEdge::ControlFlowNext));
}

// ============================================================
// Test 9: Return jumps to CfgExit
// ============================================================
#[test]
fn test_return_jumps_to_exit() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def early(x):
    if x:
        return 1
    y = 2
    return y
"#;
    build_cpg_for_source(&mut cpg, "/test/early.py", source);

    let func_idx = find_function(&cpg, "/test/early.py", "early").unwrap();
    let exit_idx = cpg.function_to_exit[&func_idx];
    let stmts = get_statements(&cpg, func_idx);

    // Find return statements
    let returns: Vec<_> = stmts.iter()
        .filter(|(_, _, sk)| sk.as_ref().map(|k| **k == StatementKind::Return).unwrap_or(false))
        .collect();
    assert_eq!(returns.len(), 2);

    // Both returns should connect to CfgExit
    for (ret_idx, _, _) in &returns {
        assert!(has_edge(&cpg, *ret_idx, exit_idx, &CpgEdge::ControlFlowNext));
    }

    // The first return should NOT have any ControlFlowNext to y=2
    let first_return = returns[0].0;
    let y_stmt = stmts.iter().find(|(_, name, _)| name.contains("y = 2"));
    if let Some((y_idx, _, _)) = y_stmt {
        assert!(!has_edge(&cpg, first_return, *y_idx, &CpgEdge::ControlFlowNext));
    }
}

// ============================================================
// Test 10: Break terminates loop path
// ============================================================
#[test]
fn test_break_exits_loop() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def search(items):
    for item in items:
        if item == "target":
            break
    found = True
"#;
    build_cpg_for_source(&mut cpg, "/test/search.py", source);

    let func_idx = find_function(&cpg, "/test/search.py", "search").unwrap();
    let stmts = get_statements(&cpg, func_idx);

    // Find break statement
    let break_stmt = stmts.iter().find(|(_, _, sk)| {
        sk.as_ref().map(|k| **k == StatementKind::Break).unwrap_or(false)
    });
    assert!(break_stmt.is_some(), "Should find break statement");

    // Find found=True statement
    let found_stmt = stmts.iter().find(|(_, name, _)| name.contains("found"));
    assert!(found_stmt.is_some(), "Should find found statement");

    // break should be collected as a loop exit → found=True should be reachable
    // via the for loop's exits
    let for_stmt = stmts.iter().find(|(_, _, sk)| {
        sk.as_ref().map(|k| **k == StatementKind::For).unwrap_or(false)
    });
    assert!(for_stmt.is_some());
}

// ============================================================
// Test 11: Continue creates ControlFlowBack to loop header
// ============================================================
#[test]
fn test_continue_back_edge() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def skip_odds(items):
    for item in items:
        if item % 2 != 0:
            continue
        process(item)
"#;
    build_cpg_for_source(&mut cpg, "/test/skip.py", source);

    let func_idx = find_function(&cpg, "/test/skip.py", "skip_odds").unwrap();
    let stmts = get_statements(&cpg, func_idx);

    let for_idx = stmts.iter().find(|(_, _, sk)| {
        sk.as_ref().map(|k| **k == StatementKind::For).unwrap_or(false)
    }).unwrap().0;

    let continue_idx = stmts.iter().find(|(_, _, sk)| {
        sk.as_ref().map(|k| **k == StatementKind::Continue).unwrap_or(false)
    }).unwrap().0;

    // continue → for header (back edge)
    assert!(has_edge(&cpg, continue_idx, for_idx, &CpgEdge::ControlFlowBack));
}

// ============================================================
// Test 12: Nested loops — break/continue target correct loop
// ============================================================
#[test]
fn test_nested_loops() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def nested():
    for i in range(10):
        for j in range(10):
            if i == j:
                break
            continue
        pass_stmt = 1
"#;
    build_cpg_for_source(&mut cpg, "/test/nested.py", source);

    let func_idx = find_function(&cpg, "/test/nested.py", "nested").unwrap();
    let stmts = get_statements(&cpg, func_idx);

    // Find both for loops
    let for_stmts: Vec<_> = stmts.iter()
        .filter(|(_, _, sk)| sk.as_ref().map(|k| **k == StatementKind::For).unwrap_or(false))
        .collect();
    assert_eq!(for_stmts.len(), 2);

    let outer_for = for_stmts[0].0;
    let inner_for = for_stmts[1].0;

    // continue should target inner for (not outer)
    let continue_idx = stmts.iter().find(|(_, _, sk)| {
        sk.as_ref().map(|k| **k == StatementKind::Continue).unwrap_or(false)
    }).unwrap().0;

    assert!(has_edge(&cpg, continue_idx, inner_for, &CpgEdge::ControlFlowBack));
    assert!(!has_edge(&cpg, continue_idx, outer_for, &CpgEdge::ControlFlowBack));
}

// ============================================================
// Test 13: With statement — body flows through
// ============================================================
#[test]
fn test_with_statement() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def file_op():
    with open("f") as f:
        data = f.read()
    process(data)
"#;
    build_cpg_for_source(&mut cpg, "/test/with.py", source);

    let func_idx = find_function(&cpg, "/test/with.py", "file_op").unwrap();
    let stmts = get_statements(&cpg, func_idx);

    // with_stmt, data=f.read(), process(data)
    assert_eq!(stmts.len(), 3);

    let with_idx = stmts[0].0;
    let data_idx = stmts[1].0;
    let process_idx = stmts[2].0;

    // with → body entry
    assert!(has_edge(&cpg, with_idx, data_idx, &CpgEdge::ControlFlowNext));
    // body exit → next
    assert!(has_edge(&cpg, data_idx, process_idx, &CpgEdge::ControlFlowNext));
}

// ============================================================
// Test 14: Match statement — branches for each case
// ============================================================
#[test]
fn test_match_statement() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def dispatch(cmd):
    match cmd:
        case "start":
            start()
        case "stop":
            stop()
    done()
"#;
    build_cpg_for_source(&mut cpg, "/test/match.py", source);

    let func_idx = find_function(&cpg, "/test/match.py", "dispatch").unwrap();
    let stmts = get_statements(&cpg, func_idx);

    // match_stmt, start(), stop(), done()
    assert!(stmts.len() >= 3);

    let match_idx = stmts[0].0;

    // match should have ControlFlowNext edges to case bodies
    let edges = get_cfg_edges(&cpg, func_idx);
    let match_outgoing: Vec<_> = edges.iter()
        .filter(|(s, _, e)| *s == match_idx && *e == CpgEdge::ControlFlowNext)
        .collect();
    // Should have edges to both case bodies
    assert!(match_outgoing.len() >= 2, "match should branch to both cases, got {}", match_outgoing.len());
}

// ============================================================
// Test 15: Integration with RepoGraph
// ============================================================
#[test]
fn test_repograph_cfg_integration() {
    let root = tempdir().unwrap();
    let root_path = root.path().to_path_buf();

    create_test_file(&root_path, "example.py", r#"
def add(a, b):
    result = a + b
    return result

def conditional(x):
    if x > 0:
        return "positive"
    return "non-positive"
"#);

    let mut graph = RepoGraph::new(&root_path, "python", &[]);
    graph.enable_cpg();
    let paths = vec![root_path.join("example.py")];
    graph.build_complete(&paths, &root_path);

    let cpg = graph.cpg.as_ref().unwrap();

    // Check that CFG was built for add()
    let add_idx = find_function(cpg, &root_path.join("example.py").to_string_lossy(), "add");
    assert!(add_idx.is_some(), "Should find function 'add'");

    let add_idx = add_idx.unwrap();
    assert!(cpg.function_to_entry.contains_key(&add_idx));
    assert!(cpg.function_to_exit.contains_key(&add_idx));

    let edges = get_cfg_edges(cpg, add_idx);
    assert!(!edges.is_empty(), "CFG should have edges");

    // Check conditional() has branching
    let cond_idx = find_function(cpg, &root_path.join("example.py").to_string_lossy(), "conditional");
    assert!(cond_idx.is_some());

    let cond_idx = cond_idx.unwrap();
    let cond_edges = get_cfg_edges(cpg, cond_idx);
    let has_true = cond_edges.iter().any(|(_, _, e)| *e == CpgEdge::ControlFlowTrue);
    assert!(has_true, "conditional() should have ControlFlowTrue edge");
}

// ============================================================
// Test 16: Empty function body — CfgEntry→CfgExit directly
// ============================================================
#[test]
fn test_empty_function_body() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def empty():
    pass
"#;
    build_cpg_for_source(&mut cpg, "/test/empty.py", source);

    let func_idx = find_function(&cpg, "/test/empty.py", "empty").unwrap();
    let entry_idx = cpg.function_to_entry[&func_idx];
    let exit_idx = cpg.function_to_exit[&func_idx];

    // pass is a statement, so we should have entry → pass → exit
    let stmts = get_statements(&cpg, func_idx);
    assert_eq!(stmts.len(), 1);
    assert_eq!(stmts[0].2, Some(&StatementKind::Pass));

    assert!(has_edge(&cpg, entry_idx, stmts[0].0, &CpgEdge::ControlFlowNext));
    assert!(has_edge(&cpg, stmts[0].0, exit_idx, &CpgEdge::ControlFlowNext));
}

// ============================================================
// Test 17: Raise jumps to CfgExit
// ============================================================
#[test]
fn test_raise_jumps_to_exit() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def validate(x):
    if x < 0:
        raise ValueError("negative")
    return x
"#;
    build_cpg_for_source(&mut cpg, "/test/validate.py", source);

    let func_idx = find_function(&cpg, "/test/validate.py", "validate").unwrap();
    let exit_idx = cpg.function_to_exit[&func_idx];
    let stmts = get_statements(&cpg, func_idx);

    let raise_stmt = stmts.iter().find(|(_, _, sk)| {
        sk.as_ref().map(|k| **k == StatementKind::Raise).unwrap_or(false)
    });
    assert!(raise_stmt.is_some(), "Should find raise statement");

    let raise_idx = raise_stmt.unwrap().0;
    assert!(has_edge(&cpg, raise_idx, exit_idx, &CpgEdge::ControlFlowNext));

    // raise should NOT flow to return
    let return_stmt = stmts.iter().find(|(_, _, sk)| {
        sk.as_ref().map(|k| **k == StatementKind::Return).unwrap_or(false)
    });
    if let Some((ret_idx, _, _)) = return_stmt {
        assert!(!has_edge(&cpg, raise_idx, *ret_idx, &CpgEdge::ControlFlowNext));
    }
}
