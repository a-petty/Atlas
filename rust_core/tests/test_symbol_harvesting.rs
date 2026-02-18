use semantic_engine::parser::{SymbolHarvester, SupportedLanguage};
use semantic_engine::test_utils::print_tree; // Add this import
use std::collections::HashSet;

// Helper to parse source code for a given language
fn parse_source_for_test(source: &str, lang: SupportedLanguage) -> tree_sitter::Tree {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(lang.get_parser().expect("Failed to get parser for language"))
        .expect("Failed to set language for parser");
    parser
        .parse(source, None)
        .expect("Failed to parse source")
}

#[test]
fn debug_python_explicit_tuple_unpacking_tree() {
    let source = "(a, b) = (1, 2)";
    let tree = parse_source_for_test(source, SupportedLanguage::Python);
    println!("\n=== PYTHON EXPLICIT TUPLE UNPACKING TREE ===");
    print_tree(tree.root_node(), source, 0);
}

#[test]
fn debug_python_unpacking_tree() {
    let source = "a, b = 1, 2";
    let tree = parse_source_for_test(source, SupportedLanguage::Python);
    println!("\n=== PYTHON UNPACKING TREE ===");
    print_tree(tree.root_node(), source, 0);
}

#[test]
fn debug_python_tree() {
    let source = r#"\
def my_function(a, b):
    pass

class MyClass:
    def __init__(self, value):
        self.value = value

GLOBAL_VAR = 10
"#;
    
    let tree = parse_source_for_test(source, SupportedLanguage::Python);
    println!("\n=== PYTHON TREE STRUCTURE ===");
    print_tree(tree.root_node(), source, 0);
}

#[test]
fn debug_rust_tree() {
    let source = r#"\
fn calculate_sum(x: i32) -> i32 {
    x + 1
}

struct MyStruct {
    field1: i32,
}

static PI: f64 = 3.14;
"#;
    
    let tree = parse_source_for_test(source, SupportedLanguage::Rust);
    println!("\n=== RUST TREE STRUCTURE ===");
    print_tree(tree.root_node(), source, 0);
}

#[test]
fn test_symbol_harvesting_python_simple() {
    let source = "def my_function(a, b):\n    pass";
    let tree = parse_source_for_test(source, SupportedLanguage::Python);
    let harvester = SymbolHarvester::new();
    let symbols = harvester.harvest(&tree, source, SupportedLanguage::Python);
    let defs: Vec<String> = symbols.iter().filter(|s| s.is_definition).map(|s| s.name.clone()).collect();
    let _uses: Vec<String> = symbols.iter().filter(|s| !s.is_definition).map(|s| s.name.clone()).collect();

    // Expected: function name (parameters are local scope, not file-level definitions)
    let expected_defs: HashSet<String> =
        ["my_function"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    
    let actual_defs: HashSet<String> = defs.into_iter().collect();

    if actual_defs != expected_defs {
        println!("MISSING: {:?}", expected_defs.difference(&actual_defs).collect::<Vec<_>>());
        println!("EXTRA: {:?}", actual_defs.difference(&expected_defs).collect::<Vec<_>>());
    }

    assert_eq!(actual_defs, expected_defs, "Simple function definition mismatch");
}

#[test]
fn test_symbol_harvesting_python_comprehensive() {
    let source = r###"import os
from sys import version_info as vinfo

class MyClass:
    def __init__(self, value):
        self.value = value

    def my_method(self, multiplier):
        result = self.value * multiplier
        return result

def my_function(a, b):
    c = a + b
    return c

GLOBAL_VAR = 10
mc = MyClass(GLOBAL_VAR)
"###;
    let tree = parse_source_for_test(source, SupportedLanguage::Python);
    let harvester = SymbolHarvester::new();
    let symbols = harvester.harvest(&tree, source, SupportedLanguage::Python);
    let defs: Vec<String> = symbols.iter().filter(|s| s.is_definition).map(|s| s.name.clone()).collect();
    let uses: Vec<String> = symbols.iter().filter(|s| !s.is_definition).map(|s| s.name.clone()).collect();

    // Core definitions we MUST capture (parameters excluded — they're local scope)
    let must_have: HashSet<String> = [
        "MyClass",      // class name
        "__init__",     // method name
        "my_method",    // method name
        "my_function",  // function name
        "GLOBAL_VAR",   // variable
        "mc",           // variable
        "c",            // variable (assignment)
        "result",       // variable (assignment)
    ].iter().map(|s| s.to_string()).collect();

    let actual_defs: HashSet<String> = defs.into_iter().collect();
    
    let missing: Vec<_> = must_have.difference(&actual_defs).collect();
    
    if !missing.is_empty() {
        println!("\nMISSING CRITICAL DEFINITIONS: {:?}", missing);
        println!("\nACTUAL DEFINITIONS: {:?}", actual_defs);
    }
    
    assert!(
        missing.is_empty(), 
        "Missing critical definitions: {:?}", 
        missing
    );
    
    assert!(!uses.is_empty(), "Usages should not be empty");
}

#[test]
fn test_query_compilation() {
    use tree_sitter::Query;
    
    // Test Python query compiles
    let py_query_str = include_str!("../queries/python/symbols.scm");
    let py_lang = SupportedLanguage::Python.get_parser().unwrap();
    let py_query = Query::new(py_lang, py_query_str);
    assert!(py_query.is_ok(), "Python query failed to compile: {:?}", py_query.err());
    
    // Test Rust query compiles
    let rs_query_str = include_str!("../queries/rust/symbols.scm");
    let rs_lang = SupportedLanguage::Rust.get_parser().unwrap();
    let rs_query = Query::new(rs_lang, rs_query_str);
    assert!(rs_query.is_ok(), "Rust query failed to compile: {:?}", rs_query.err());
    
    println!("✓ Both queries compile successfully");
}