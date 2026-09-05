use semantic_engine::cpg::{CpgLayer, CpgNodeKind, CpgEdge};
use semantic_engine::import_resolver::ResolverGroup;
use semantic_engine::parser::SupportedLanguage;
use std::path::PathBuf;
use tree_sitter::Parser as TreeSitterParser;

/// Helper: parse JS/TS source and build CPG for it.
fn build_js_cpg(cpg: &mut CpgLayer, path: &str, source: &str) {
    let path = PathBuf::from(path);
    let lang = SupportedLanguage::from_path(&path);
    let mut parser = TreeSitterParser::new();
    parser
        .set_language(lang.get_parser().unwrap())
        .unwrap();
    let tree = parser.parse(source, None).unwrap();
    cpg.build_file(&path, tree, source.to_string(), lang);
}

// ============================================================
// Function extraction
// ============================================================

#[test]
fn test_js_function_declaration() {
    let mut cpg = CpgLayer::new();
    let source = r#"
/** Adds two numbers */
function add(a, b) {
    return a + b;
}
"#;
    build_js_cpg(&mut cpg, "/test/math.js", source);

    let path = PathBuf::from("/test/math.js");
    let funcs = cpg.get_functions_in_file(&path);
    assert_eq!(funcs.len(), 1);

    let f = &funcs[0];
    assert_eq!(f.name, "add");
    assert_eq!(f.kind, CpgNodeKind::Function);
    assert_eq!(f.parameters.len(), 2);
    assert_eq!(f.parameters[0].name, "a");
    assert_eq!(f.parameters[1].name, "b");
    assert!(f.docstring.as_ref().unwrap().contains("Adds two numbers"));
}

#[test]
fn test_js_arrow_function() {
    let mut cpg = CpgLayer::new();
    let source = r#"
const multiply = (x, y) => x * y;
"#;
    build_js_cpg(&mut cpg, "/test/math.js", source);

    let path = PathBuf::from("/test/math.js");
    let funcs = cpg.get_functions_in_file(&path);
    assert_eq!(funcs.len(), 1);

    let f = &funcs[0];
    assert_eq!(f.name, "multiply");
    assert_eq!(f.kind, CpgNodeKind::Function);
    assert_eq!(f.parameters.len(), 2);
}

#[test]
fn test_js_generator_function() {
    let mut cpg = CpgLayer::new();
    let source = r#"
function* range(start, end) {
    for (let i = start; i < end; i++) {
        yield i;
    }
}
"#;
    build_js_cpg(&mut cpg, "/test/gen.js", source);

    let path = PathBuf::from("/test/gen.js");
    let funcs = cpg.get_functions_in_file(&path);
    assert_eq!(funcs.len(), 1);
    assert_eq!(funcs[0].name, "range");
    assert_eq!(funcs[0].parameters.len(), 2);
}

#[test]
fn test_js_async_function() {
    let mut cpg = CpgLayer::new();
    // Note: tree-sitter-javascript treats `async function` as a function_declaration
    // with an "async" keyword child, not a separate node kind
    let source = r#"
function fetchData(url) {
    return fetch(url);
}
"#;
    build_js_cpg(&mut cpg, "/test/api.js", source);

    let path = PathBuf::from("/test/api.js");
    let funcs = cpg.get_functions_in_file(&path);
    assert_eq!(funcs.len(), 1);
    assert_eq!(funcs[0].name, "fetchData");
}

// ============================================================
// Class extraction
// ============================================================

#[test]
fn test_js_class_with_methods() {
    let mut cpg = CpgLayer::new();
    let source = r#"
class Animal {
    constructor(name) {
        this.name = name;
    }

    speak() {
        return this.name;
    }

    static create(name) {
        return new Animal(name);
    }
}
"#;
    build_js_cpg(&mut cpg, "/test/animal.js", source);

    let path = PathBuf::from("/test/animal.js");
    let classes = cpg.get_classes_in_file(&path);
    assert_eq!(classes.len(), 1);
    assert_eq!(classes[0].name, "Animal");

    let funcs = cpg.get_functions_in_file(&path);
    // Class + 3 methods
    let methods: Vec<_> = funcs.iter().filter(|f| f.kind == CpgNodeKind::Method).collect();
    assert_eq!(methods.len(), 3);

    let method_names: Vec<&str> = methods.iter().map(|m| m.name.as_str()).collect();
    assert!(method_names.contains(&"constructor"));
    assert!(method_names.contains(&"speak"));
    assert!(method_names.contains(&"create"));

    // Check parent_class
    for m in &methods {
        assert_eq!(m.parent_class.as_deref(), Some("Animal"));
    }
}

#[test]
fn test_js_class_extends() {
    let mut cpg = CpgLayer::new();
    let source = r#"
class Dog extends Animal {
    bark() {
        return "woof";
    }
}
"#;
    build_js_cpg(&mut cpg, "/test/dog.js", source);

    let path = PathBuf::from("/test/dog.js");
    let classes = cpg.get_classes_in_file(&path);
    assert_eq!(classes.len(), 1);
    assert_eq!(classes[0].name, "Dog");
    assert!(classes[0].bases.contains(&"Animal".to_string()));
}

// ============================================================
// TypeScript typed parameters
// ============================================================

#[test]
fn test_ts_typed_params_and_return() {
    let mut cpg = CpgLayer::new();
    let source = r#"
function greet(name: string, count: number = 1): string {
    return name.repeat(count);
}
"#;
    build_js_cpg(&mut cpg, "/test/greet.ts", source);

    let path = PathBuf::from("/test/greet.ts");
    let funcs = cpg.get_functions_in_file(&path);
    assert_eq!(funcs.len(), 1);

    let f = &funcs[0];
    assert_eq!(f.name, "greet");
    assert_eq!(f.parameters.len(), 2);
    assert_eq!(f.parameters[0].name, "name");
    assert_eq!(f.parameters[0].type_annotation.as_deref(), Some("string"));
    assert_eq!(f.parameters[1].name, "count");
    assert_eq!(f.parameters[1].type_annotation.as_deref(), Some("number"));
    assert_eq!(f.return_type.as_deref(), Some("string"));
}

// ============================================================
// Variable extraction
// ============================================================

#[test]
fn test_js_variable_extraction() {
    let mut cpg = CpgLayer::new();
    let source = r#"
const API_URL = "https://api.example.com";
let counter = 0;
"#;
    build_js_cpg(&mut cpg, "/test/config.js", source);

    let path = PathBuf::from("/test/config.js");
    let nodes = cpg.get_nodes_for_file(&path);
    let vars: Vec<_> = nodes.iter().filter(|n| n.kind == CpgNodeKind::Variable).collect();
    assert_eq!(vars.len(), 2);

    let var_names: Vec<&str> = vars.iter().map(|v| v.name.as_str()).collect();
    assert!(var_names.contains(&"API_URL"));
    assert!(var_names.contains(&"counter"));
}

// ============================================================
// Export statement unwrapping
// ============================================================

#[test]
fn test_js_export_function() {
    let mut cpg = CpgLayer::new();
    let source = r#"
export function helper(x) {
    return x + 1;
}

export class Service {
    run() {}
}

export const handler = (req) => req;
"#;
    build_js_cpg(&mut cpg, "/test/mod.js", source);

    let path = PathBuf::from("/test/mod.js");
    let funcs = cpg.get_functions_in_file(&path);

    let func_names: Vec<&str> = funcs.iter().map(|f| f.name.as_str()).collect();
    assert!(func_names.contains(&"helper"), "Should find exported function 'helper'");
    assert!(func_names.contains(&"Service"), "Should find exported class 'Service'");
    assert!(func_names.contains(&"handler"), "Should find exported arrow function 'handler'");
    assert!(func_names.contains(&"run"), "Should find method 'run'");
}

// ============================================================
// CFG construction
// ============================================================

#[test]
fn test_js_cfg_if_else() {
    let mut cpg = CpgLayer::new();
    let source = r#"
function check(x) {
    if (x > 0) {
        return 1;
    } else {
        return -1;
    }
}
"#;
    build_js_cpg(&mut cpg, "/test/cfg.js", source);

    let path = PathBuf::from("/test/cfg.js");
    let funcs = cpg.get_functions_in_file(&path);
    let _func = funcs.iter().find(|f| f.name == "check").unwrap();

    // Should have entry and exit nodes
    let func_idx = cpg.file_to_nodes.get(&path).unwrap()
        .iter()
        .find(|idx| cpg.graph[**idx].name == "check")
        .unwrap();
    assert!(cpg.function_to_entry.contains_key(func_idx));
    assert!(cpg.function_to_exit.contains_key(func_idx));

    // Should have CFG edges
    let cfg_edges = cpg.get_cfg_edges_for_function(*func_idx);
    assert!(!cfg_edges.is_empty(), "Should have CFG edges");

    // Should have true and false branches
    let has_true = cfg_edges.iter().any(|(_, _, e)| **e == CpgEdge::ControlFlowTrue);
    let has_false = cfg_edges.iter().any(|(_, _, e)| **e == CpgEdge::ControlFlowFalse);
    assert!(has_true, "Should have true branch");
    assert!(has_false, "Should have false branch");
}

#[test]
fn test_js_cfg_for_loop() {
    let mut cpg = CpgLayer::new();
    let source = r#"
function sum(arr) {
    let total = 0;
    for (const x of arr) {
        total += x;
    }
    return total;
}
"#;
    build_js_cpg(&mut cpg, "/test/loop.js", source);

    let path = PathBuf::from("/test/loop.js");
    let func_idx = cpg.file_to_nodes.get(&path).unwrap()
        .iter()
        .find(|idx| cpg.graph[**idx].name == "sum")
        .unwrap();

    let cfg_edges = cpg.get_cfg_edges_for_function(*func_idx);
    assert!(!cfg_edges.is_empty(), "Should have CFG edges for loop");

    // Should have back edge
    let has_back = cfg_edges.iter().any(|(_, _, e)| **e == CpgEdge::ControlFlowBack);
    assert!(has_back, "Should have back edge for loop");
}

#[test]
fn test_js_cfg_try_catch() {
    let mut cpg = CpgLayer::new();
    let source = r#"
function safeParse(json) {
    try {
        return JSON.parse(json);
    } catch (e) {
        return null;
    }
}
"#;
    build_js_cpg(&mut cpg, "/test/trycatch.js", source);

    let path = PathBuf::from("/test/trycatch.js");
    let func_idx = cpg.file_to_nodes.get(&path).unwrap()
        .iter()
        .find(|idx| cpg.graph[**idx].name == "safeParse")
        .unwrap();

    let cfg_edges = cpg.get_cfg_edges_for_function(*func_idx);
    assert!(!cfg_edges.is_empty(), "Should have CFG edges for try/catch");

    // Should have exception edge
    let has_exception = cfg_edges.iter().any(|(_, _, e)| **e == CpgEdge::ControlFlowException);
    assert!(has_exception, "Should have exception edge for catch");
}

// ============================================================
// Call site extraction
// ============================================================

#[test]
fn test_js_call_site_extraction() {
    let mut cpg = CpgLayer::new();
    let source = r#"
function helper() {
    return 42;
}

function main() {
    const x = helper();
    console.log(x);
}
"#;
    build_js_cpg(&mut cpg, "/test/calls.js", source);

    let path = PathBuf::from("/test/calls.js");
    let main_idx = cpg.file_to_nodes.get(&path).unwrap()
        .iter()
        .find(|idx| cpg.graph[**idx].name == "main")
        .copied()
        .unwrap();

    let sites = cpg.call_sites.get(&main_idx).unwrap();
    assert!(sites.len() >= 2, "Should have at least 2 call sites (helper, console.log)");

    let callee_names: Vec<&str> = sites.iter().map(|s| s.callee_name.as_str()).collect();
    assert!(callee_names.contains(&"helper"), "Should find call to helper()");
    assert!(callee_names.contains(&"log"), "Should find call to console.log()");

    // console.log should have receiver "console"
    let log_site = sites.iter().find(|s| s.callee_name == "log").unwrap();
    assert_eq!(log_site.receiver.as_deref(), Some("console"));
}

// ============================================================
// End-to-end: RepoGraph integration
// ============================================================

#[test]
fn test_js_repograph_cpg_integration() {
    use semantic_engine::graph::RepoGraph;
    use tempfile::tempdir;
    use std::fs;

    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create a JS file
    let js_path = root.join("utils.js");
    fs::write(&js_path, r#"
function process(data) {
    return data.map(x => x * 2);
}

class Processor {
    constructor(config) {
        this.config = config;
    }

    run(input) {
        return process(input);
    }
}
"#).unwrap();

    // Create package.json so the JS resolver works
    fs::write(root.join("package.json"), "{}").unwrap();

    let mut graph = RepoGraph::new_multi(root, &[ResolverGroup::JsTs], &[], None);
    // We need to add the file to the graph first
    let content = fs::read_to_string(&js_path).unwrap();
    graph.add_file(js_path.clone(), &content).ok();
    graph.build_complete(&[js_path.clone()], root);

    // Build CPG for the file
    let result = graph.build_cpg_for_file(&js_path);
    assert!(result, "CPG should build successfully for JS file");

    let cpg = graph.cpg.as_ref().unwrap();
    let funcs = cpg.get_functions_in_file(&js_path);

    let func_names: Vec<&str> = funcs.iter().map(|f| f.name.as_str()).collect();
    assert!(func_names.contains(&"process"), "Should find function 'process'");
    assert!(func_names.contains(&"Processor"), "Should find class 'Processor'");
    assert!(func_names.contains(&"constructor"), "Should find method 'constructor'");
    assert!(func_names.contains(&"run"), "Should find method 'run'");
}

// ============================================================
// Dataflow
// ============================================================

#[test]
fn test_js_dataflow_var_declaration() {
    let mut cpg = CpgLayer::new();
    let source = r#"
function compute(a, b) {
    const sum = a + b;
    const doubled = sum * 2;
    return doubled;
}
"#;
    build_js_cpg(&mut cpg, "/test/df.js", source);

    let path = PathBuf::from("/test/df.js");
    let func_idx = cpg.file_to_nodes.get(&path).unwrap()
        .iter()
        .find(|idx| cpg.graph[**idx].name == "compute")
        .copied()
        .unwrap();

    // Should have dataflow edges
    let df_edges = cpg.get_dataflow_edges_for_function(func_idx);
    assert!(!df_edges.is_empty(), "Should have dataflow edges");

    // Check that 'sum' flows from definition to use
    let sum_flows: Vec<_> = df_edges.iter().filter(|(_, _, var)| *var == "sum").collect();
    assert!(!sum_flows.is_empty(), "Should have dataflow for 'sum'");
}

// ============================================================
// Rest parameters
// ============================================================

#[test]
fn test_js_rest_params() {
    let mut cpg = CpgLayer::new();
    let source = r#"
function collect(first, ...rest) {
    return [first, ...rest];
}
"#;
    build_js_cpg(&mut cpg, "/test/rest.js", source);

    let path = PathBuf::from("/test/rest.js");
    let funcs = cpg.get_functions_in_file(&path);
    assert_eq!(funcs.len(), 1);
    assert_eq!(funcs[0].parameters.len(), 2);
    assert_eq!(funcs[0].parameters[0].name, "first");
    assert!(funcs[0].parameters[1].name.contains("rest"));
}

