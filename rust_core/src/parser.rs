use std::collections::{HashMap, HashSet};
use tree_sitter::{Language, Parser, Query, QueryCursor, Tree};
use pyo3::prelude::*;

#[pyfunction]
pub fn create_skeleton_from_source(source: &str, lang_ext: &str) -> PyResult<String> {
    let lang = SupportedLanguage::from_extension(lang_ext);
    if lang == SupportedLanguage::Unknown {
        // Fallback for unsupported languages: return the original source
        return Ok(source.to_string());
    }

    let mut parser = Parser::new();
    let ts_lang = lang.get_parser().ok_or_else(|| pyo3::exceptions::PyValueError::new_err("Could not get parser for language"))?;
    parser.set_language(ts_lang).map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("Failed to set language: {}", e)))?;

    let tree = parser.parse(source, None).ok_or_else(|| pyo3::exceptions::PyValueError::new_err("Failed to parse source"))?;

    Ok(create_skeleton(source, &tree, lang))
}

/// Represents the result of a parsing operation, which may be partial.
pub struct ParseResult {
    pub tree: Option<Tree>,
    pub has_errors: bool,
    pub warning_message: Option<String>,
}

impl ParseResult {
    /// Creates an empty result with a warning message.
    fn empty_with_warning(msg: &str) -> Self {
        Self {
            tree: None,
            has_errors: false,
            warning_message: Some(msg.to_string()),
        }
    }
    
    /// Creates an empty result with an error message.
    fn empty_with_error(msg: &str) -> Self {
        Self {
            tree: None,
            has_errors: true,
            warning_message: Some(msg.to_string()),
        }
    }
}


/// Parses source code with graceful error handling. If Tree-sitter encounters
/// syntax errors, it will still produce a tree, but `has_errors` will be true.
/// If parsing fails completely, it returns a result with no tree.
pub fn parse_with_fallback(
    source: &str,
    language: SupportedLanguage
) -> Result<ParseResult, &'static str> {
    if language == SupportedLanguage::Unknown {
        return Ok(ParseResult::empty_with_warning("Unknown language"));
    }
    
    // This is inefficient as it creates a new pool every time.
    // In a real scenario, this would take a mutable reference to a ParserPool.
    // For this implementation, we'll stick to the isolated function pattern.
    let mut pool = ParserPool::new();
    let parser = pool.get(language)
        .ok_or("Unsupported language")?;
    
    match parser.parse(source, None) {
        Some(tree) => {
            let has_errors = tree.root_node().has_error();
            if has_errors {
                log::warn!("Syntax errors detected in file, but proceeding with partial tree.");
            }
            Ok(ParseResult {
                tree: Some(tree),
                has_errors,
                warning_message: if has_errors { Some("Partial parse due to syntax errors".to_string()) } else { None },
            })
        },
        None => {
            log::error!("Tree-sitter parse failed completely.");
            Ok(ParseResult::empty_with_error("Complete parse failure"))
        }
    }
}


#[pyclass]
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub enum SupportedLanguage {
    Python,
    Rust,
    JavaScript,
    JavaScriptJsx,
    TypeScript,
    TypeScriptTsx,
    Go,
    Unknown,
}

use std::path::Path;
impl SupportedLanguage {
    pub fn from_path(path: &Path) -> Self {
        path.extension()
            .and_then(|s| s.to_str())
            .map(Self::from_extension)
            .unwrap_or(Self::Unknown)
    }

    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "py" => Self::Python,
            "rs" => Self::Rust,
            "js" | "mjs" | "cjs" => Self::JavaScript,
            "jsx" => Self::JavaScriptJsx,
            "ts" => Self::TypeScript,
            "tsx" => Self::TypeScriptTsx,
            "go" => Self::Go,
            _ => Self::Unknown,
        }
    }

    pub fn get_parser(&self) -> Option<Language> {
        match self {
            Self::Python => Some(tree_sitter_python::language()),
            Self::Rust => Some(tree_sitter_rust::language()),
            Self::JavaScript => Some(tree_sitter_javascript::language()),
            Self::JavaScriptJsx => Some(tree_sitter_javascript::language()), // tree-sitter-javascript handles JSX
            Self::TypeScript => Some(tree_sitter_typescript::language_typescript()),
            Self::TypeScriptTsx => Some(tree_sitter_typescript::language_tsx()),
            Self::Go => Some(tree_sitter_go::language()),
            Self::Unknown => None,
        }
    }

    pub fn all() -> &'static [SupportedLanguage] {
        &[
            SupportedLanguage::Python,
            SupportedLanguage::Rust,
            SupportedLanguage::JavaScript,
            SupportedLanguage::JavaScriptJsx,
            SupportedLanguage::TypeScript,
            SupportedLanguage::TypeScriptTsx,
            SupportedLanguage::Go,
        ]
    }

    /// Stable string name for cross-boundary serialization (Python dict keys,
    /// MCP rendering). Distinct from `Debug` formatting because consumers
    /// rely on these strings — changing them is a breaking API change.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Rust => "rust",
            Self::JavaScript => "javascript",
            Self::JavaScriptJsx => "javascript_jsx",
            Self::TypeScript => "typescript",
            Self::TypeScriptTsx => "typescript_tsx",
            Self::Go => "go",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Interface,
    Type,
    Enum,
    Variable,
}

/// Represents a symbol (definition or usage) found in source code
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub start_byte: usize,
    pub end_byte: usize,
    pub is_definition: bool,  // true = definition, false = reference/usage
}

fn parse_capture_name(capture_name: &str) -> (SymbolKind, bool) {
    let parts: Vec<&str> = capture_name.split('.').collect();
    
    let is_definition = match parts.get(0) {
        Some(&"definition") | Some(&"def") => true,
        _ => false,
    };
    
    let kind_str = if parts.len() > 1 { parts[1] } else { "" };

    let kind = match kind_str {
        "function" => SymbolKind::Function,
        "method" => SymbolKind::Method,
        "class" => SymbolKind::Class,
        "interface" => SymbolKind::Interface,
        "type" => SymbolKind::Type,
        "enum" => SymbolKind::Enum,
        "variable" => SymbolKind::Variable,
        // Fallback for `call` and `reference` which don't have a specific kind in the new enum
        _ => SymbolKind::Variable, 
    };

    (kind, is_definition)
}

const TRIVIAL_PYTHON_NODES: &[&str] = &[
    "module",
    "block",
    "expression_statement",
    "parameters",
    "argument_list",
    "parenthesized_expression",
    "tuple_pattern",
    "list_pattern",
    "pattern_list",
];

const TRIVIAL_RUST_NODES: &[&str] = &[
    "token_tree", 
    "source_file", 
    "block"
];

const TRIVIAL_JS_TS_NODES: &[&str] = &[
    "program", 
    "expression_statement",
];

const TRIVIAL_GO_NODES: &[&str] = &[
    "source_file", 
    "expression_statement",
];

fn get_trivial_nodes(language: SupportedLanguage) -> &'static [&'static str] {
    match language {
        SupportedLanguage::Python => &TRIVIAL_PYTHON_NODES,
        SupportedLanguage::Rust => &TRIVIAL_RUST_NODES,
        SupportedLanguage::JavaScript | SupportedLanguage::TypeScript | SupportedLanguage::JavaScriptJsx | SupportedLanguage::TypeScriptTsx => &TRIVIAL_JS_TS_NODES,
        SupportedLanguage::Go => &TRIVIAL_GO_NODES,
        _ => &[],
    }
}

#[derive(Debug)]
pub struct NormalizedNode {
    pub kind: String,
    pub text: Option<String>,
    pub children: Vec<NormalizedNode>,
    pub byte_range: (usize, usize),
}

pub fn normalize_tree(root: tree_sitter::Node, source: &[u8], language: SupportedLanguage) -> NormalizedNode {
    let trivial_nodes = get_trivial_nodes(language);
    let children = normalize_node_impl(root, source, trivial_nodes);

    if children.len() == 1 {
        children.into_iter().next().unwrap()
    } else {
        NormalizedNode {
            kind: root.kind().to_string(),
            text: None,
            children,
            byte_range: (root.start_byte(), root.end_byte()),
        }
    }
}

fn normalize_node_impl(
    node: tree_sitter::Node,
    source: &[u8],
    trivial_nodes: &[&str]
) -> Vec<NormalizedNode> {
    let kind = node.kind();
    let is_trivial = trivial_nodes.contains(&kind);
    
    let mut processed_children = Vec::new();
    for i in 0..node.named_child_count() {
        if let Some(child) = node.named_child(i) {
            processed_children.extend(normalize_node_impl(child, source, trivial_nodes));
        }
    }
    
    if is_trivial && !processed_children.is_empty() {
        return processed_children;
    }
    
    let text = if processed_children.is_empty() {
        Some(node.utf8_text(source).unwrap_or("").to_string())
    } else {
        None
    };
    
    vec![NormalizedNode {
        kind: kind.to_string(),
        text,
        children: processed_children,
        byte_range: (node.start_byte(), node.end_byte()),
    }]
}

#[derive(Debug, Default)]
pub struct FunctionSignature {
    pub name: String,
    pub params: String,
    pub return_type: Option<String>,
    pub docstring: Option<String>,
    pub body_byte_range: (usize, usize),
}

#[deprecated(since = "0.2.0", note = "Use create_skeleton(source, tree, lang) instead")]
pub fn extract_signatures(
    tree: &Tree,
    source: &str,
    language: Language
) -> Vec<FunctionSignature> {
    let query_source = include_str!("../queries/python/tags.scm");
    let query = Query::new(language, query_source).unwrap();
    
    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    
    let mut signatures = Vec::new();
    let capture_names = query.capture_names();

    for m in matches {
        if m.pattern_index != 0 {
            continue;
        }
        let mut sig = FunctionSignature::default();
        
        for capture in m.captures {
            let capture_name = &capture_names[capture.index as usize];
            let text = capture.node.utf8_text(source.as_bytes()).unwrap();
            
            match capture_name.as_str() {
                "function.name" => sig.name = text.to_string(),
                "function.params" => sig.params = text.to_string(),
                "function.return" => sig.return_type = Some(text.to_string()),
                "function.doc" => sig.docstring = Some(text.to_string()),
                "function.body" => {
                    sig.body_byte_range = (
                        capture.node.byte_range().start,
                        capture.node.byte_range().end
                    );
                }
                _ => {}
            }
        }
        
        if !sig.name.is_empty() {
            signatures.push(sig);
        }
    }
    
    signatures
}

pub fn create_skeleton(source: &str, tree: &Tree, language: SupportedLanguage) -> String {
    if language != SupportedLanguage::Python {
        return source.to_string();
    }
    
    let mut kept_ranges: Vec<(usize, usize, Option<String>)> = Vec::new();
    collect_keeper_ranges(tree.root_node(), source, &mut kept_ranges);
    
    kept_ranges.sort_by_key(|r| r.0);
    
    let mut result = String::new();
    let mut last_end = 0;
    
    for (_i, (start, end, replacement)) in kept_ranges.iter().enumerate() {
        if *start > last_end {
            let gap = &source[last_end..*start];
            let newline_count = gap.chars().filter(|&c| c == '\n').count();
            
            if newline_count > 0 {
                result.push('\n');
                if newline_count > 1 {
                    result.push('\n');
                }
            }
        }
        
        if let Some(repl) = replacement {
            result.push_str(repl);
        } else {
            result.push_str(&source[*start..*end]);
        }
        
        last_end = *end;
    }
    
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }
    
    result
}

fn collect_keeper_ranges(
    node: tree_sitter::Node, 
    source: &str,
    ranges: &mut Vec<(usize, usize, Option<String>)>
) {
    match node.kind() {
        "module" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_keeper_ranges(child, source, ranges);
            }
        }
        
        "import_statement" | "import_from_statement" => {
            ranges.push((node.start_byte(), node.end_byte(), None));
        }
        
        "function_definition" | "async_function_definition" => {
            handle_function(node, source, ranges);
        }
        
        "class_definition" => {
            handle_class(node, source, ranges);
        }
        
        "decorated_definition" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "function_definition" 
                   || child.kind() == "class_definition"
                   || child.kind() == "async_function_definition" {
                    collect_keeper_ranges(child, source, ranges);
                }
            }
        }
        
        _ => {}
    }
}

// CRITICAL FIX: Place ellipsis immediately after last kept content
fn handle_function(
    node: tree_sitter::Node, 
    source: &str,
    ranges: &mut Vec<(usize, usize, Option<String>)>
) {
    if let Some(body) = node.child_by_field_name("body") {
        let sig_start = node.start_byte();
        let sig_end = body.start_byte();
        
        // Keep signature (up to and including ':')
        ranges.push((sig_start, sig_end, None));
        
        // Get proper indentation
        let indent = get_indent_from_body(body, source);
        
        // Check for docstring
        if let Some(first_stmt) = body.named_child(0) {
            if is_docstring(first_stmt) {
                // Keep docstring
                ranges.push((first_stmt.start_byte(), first_stmt.end_byte(), None));
                
                // CRITICAL: Add ellipsis RIGHT AFTER docstring to prevent body code from being kept
                ranges.push((first_stmt.end_byte(), first_stmt.end_byte(), Some(format!("\n{}...", indent))));
                return; // Early return - we're done with this function
            }
        }
        
        // No docstring - add ellipsis right after signature
        ranges.push((sig_end, sig_end, Some(format!("\n{}...", indent))));
    } else {
        // No body
        ranges.push((node.start_byte(), node.end_byte(), None));
    }
}

fn handle_class(
    node: tree_sitter::Node, 
    source: &str,
    ranges: &mut Vec<(usize, usize, Option<String>)>
) {
    if let Some(body) = node.child_by_field_name("body") {
        let header_start = node.start_byte();
        let header_end = body.start_byte();
        
        // Keep class header
        ranges.push((header_start, header_end, None));
        
        // Check for class docstring
        if let Some(first_stmt) = body.named_child(0) {
            if is_docstring(first_stmt) {
                ranges.push((first_stmt.start_byte(), first_stmt.end_byte(), None));
            }
        }
        
        // Process methods in class body
        let mut cursor = body.walk();
        for (_idx, child) in body.children(&mut cursor).enumerate() {
            if !child.is_named() {
                continue;
            }
            
            // Skip docstring (it was first child)
            if is_docstring(child) {
                continue;
            }
            
            match child.kind() {
                "function_definition" | "async_function_definition" => {
                    handle_function(child, source, ranges);
                }
                "decorated_definition" => {
                    collect_keeper_ranges(child, source, ranges);
                }
                _ => {}
            }
        }
    } else {
        ranges.push((node.start_byte(), node.end_byte(), None));
    }
}

fn is_docstring(node: tree_sitter::Node) -> bool {
    if node.kind() != "expression_statement" {
        return false;
    }
    
    if let Some(child) = node.named_child(0) {
        child.kind() == "string" || child.kind() == "concatenated_string"
    } else {
        false
    }
}

fn get_indent_from_body(body: tree_sitter::Node, source: &str) -> String {
    if let Some(first) = body.named_child(0) {
        let line_start = source[..first.start_byte()]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let indent_str = &source[line_start..first.start_byte()];
        indent_str.to_string()
    } else {
        "    ".to_string()
    }
}

#[derive(Debug)]
pub struct SymbolHarvester {
    python_query: Query,
    javascript_query: Query,
    typescript_query: Query,
}

impl SymbolHarvester {
    pub fn new() -> Self {
        Self {
            python_query: Query::new(
                tree_sitter_python::language(),
                include_str!("../queries/python/symbols.scm"),
            )
            .expect("Failed to load Python symbols query"),
            javascript_query: Query::new(
                tree_sitter_javascript::language(),
                include_str!("../queries/javascript/symbols.scm"),
            )
            .expect("Failed to load JavaScript symbols query"),
            typescript_query: Query::new(
                tree_sitter_typescript::language_typescript(),
                include_str!("../queries/typescript/symbols.scm"),
            )
            .expect("Failed to load TypeScript symbols query"),
        }
    }

    pub fn harvest(&self, tree: &Tree, source: &str, lang: SupportedLanguage) -> Vec<Symbol> {
        let query = self.select_query(lang);
        let mut symbols = Vec::new();
        let mut cursor = QueryCursor::new();
        
        let source_bytes = source.as_bytes();

        for match_ in cursor.matches(query, tree.root_node(), source_bytes) {
            for capture in match_.captures {
                let capture_name = &query.capture_names()[capture.index as usize];
                let node = capture.node;
                
                if let Ok(name) = node.utf8_text(source_bytes) {
                    let (kind, is_definition) = parse_capture_name(capture_name);
                    
                    symbols.push(Symbol {
                        name: name.to_string(),
                        kind,
                        start_byte: node.start_byte(),
                        end_byte: node.end_byte(),
                        is_definition,
                    });
                }
            }
        }
        
        symbols
    }

    fn select_query(&self, lang: SupportedLanguage) -> &Query {
        match lang {
            SupportedLanguage::Python => &self.python_query,
            SupportedLanguage::JavaScript | SupportedLanguage::JavaScriptJsx => &self.javascript_query,
            SupportedLanguage::TypeScript | SupportedLanguage::TypeScriptTsx => &self.typescript_query,
            _ => &self.python_query, // Fallback, consider logging a warning
        }
    }
}

#[pyclass]
pub struct ParserPool {
    parsers: HashMap<SupportedLanguage, Parser>,
}

#[pymethods]
impl ParserPool {
    #[new]
    pub fn new_py() -> Self {
        Self::new()
    }

    pub fn num_parsers(&self) -> usize {
        self.parsers.len()
    }
}

impl Default for SymbolHarvester {
    fn default() -> Self {
        Self::new()
    }
}

impl ParserPool {
    fn new() -> Self {
        let mut pool = Self {
            parsers: HashMap::new(),
        };

        for lang in SupportedLanguage::all() {
            if let Some(ts_lang) = lang.get_parser() {
                let mut parser = Parser::new();
                if let Err(e) = parser.set_language(ts_lang) {
                    eprintln!("Warning: Failed to load parser for {:?}: {}", lang, e);
                    continue;
                }
                pool.parsers.insert(*lang, parser);
            }
        }

        pool
    }

    pub fn get(&mut self, lang: SupportedLanguage) -> Option<&mut Parser> {
        self.parsers.get_mut(&lang)
    }
}

#[cfg(test)]
use std::sync::Mutex;
#[cfg(test)]
static TEST_PARSER_POOL: Mutex<Option<ParserPool>> = Mutex::new(None);

#[cfg(test)]
fn get_test_parser_pool() -> &'static Mutex<Option<ParserPool>> {
    let mut pool = TEST_PARSER_POOL.lock().unwrap();
    if pool.is_none() {
        *pool = Some(ParserPool::new());
    }
    &TEST_PARSER_POOL
}

#[cfg(test)]
fn parse_source(source: &str, lang: SupportedLanguage) -> Tree {
    let mut pool_guard = get_test_parser_pool().lock().unwrap();
    let pool = pool_guard.as_mut().unwrap();
    
    let parser = pool.get(lang).expect("Parser not found for language");
    parser.parse(source, None).expect("Failed to parse parse source")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::print_tree;

    fn count_nodes(node: &NormalizedNode) -> usize {
        1 + node.children.iter().map(count_nodes).sum::<usize>()
    }

    #[test]
    fn test_normalization_reduces_nodes() {
        let source = "x = 5";
        let tree = parse_source(source, SupportedLanguage::Python);
        let normalized = normalize_tree(tree.root_node(), source.as_bytes(), SupportedLanguage::Python);
        
        assert!(count_nodes(&normalized) < 5); 
        assert_eq!(normalized.kind, "assignment"); 
    }

    #[test]
    fn debug_python_tree() {
        let source = r#"\
def my_function(a, b):\
    pass\
\
class MyClass:\
    def __init__(self, value):\
        self.value = value\
\
GLOBAL_VAR = 10\
"#;
        let tree = parse_source(source, SupportedLanguage::Python);
        println!("\n=== PYTHON TREE STRUCTURE ===");
        print_tree(tree.root_node(), source, 0);
    }

    #[test]
    fn debug_rust_tree() {
        let source = r#"\
fn calculate_sum(x: i32) -> i32 {\
    x + 1\
}\
\
struct MyStruct {\
    field1: i32,\
}\
\
static PI: f64 = 3.14;\
"#;
        let tree = parse_source(source, SupportedLanguage::Rust);
        println!("\n=== RUST TREE STRUCTURE ===");
        print_tree(tree.root_node(), source, 0);
    }
}

#[cfg(test)]
mod tree_inspection {
    use super::*;
    use crate::test_utils::print_tree;
    
    #[test]
    fn inspect_python_function() {
        let source = "def my_function(a, b):\n    pass";
        let tree = parse_source(source, SupportedLanguage::Python);
        println!("\n=== PYTHON FUNCTION TREE ===");
        print_tree(tree.root_node(), source, 0);
    }
    
    #[test]
    fn inspect_rust_function() {
        let source = "fn calculate_sum(x: i32, y: i32) -> i32 {\n    x + y\n}";
        let tree = parse_source(source, SupportedLanguage::Rust);
        println!("\n=== RUST FUNCTION TREE ===");
        print_tree(tree.root_node(), source, 0);
    }
}