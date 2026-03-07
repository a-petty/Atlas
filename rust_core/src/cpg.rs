use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tree_sitter::Tree;

use crate::import_resolver::ImportBinding;
use crate::parser::SupportedLanguage;

/// The kind of a CPG node (sub-file granularity).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpgNodeKind {
    Function,
    Method,
    Class,
    Variable,
    Statement,  // Individual statement within a function
    CfgEntry,   // Sentinel for function entry point
    CfgExit,    // Sentinel for function exit point
}

/// Classification of a statement for CFG construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatementKind {
    If,
    For,
    While,
    Try,
    With,
    Match,
    Return,
    Break,
    Continue,
    Raise,
    Assignment,
    ExpressionStatement,
    Assert,
    Pass,
    Import,
    Delete,
    Other(String),
}

/// A parameter of a function or method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub name: String,
    pub type_annotation: Option<String>,
    pub default_value: Option<String>,
}

/// A fine-grained node representing a symbol within a file.
#[derive(Debug, Clone)]
pub struct CpgNode {
    pub file_path: PathBuf,
    pub name: String,
    pub kind: CpgNodeKind,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub parameters: Vec<Parameter>,
    pub return_type: Option<String>,
    pub docstring: Option<String>,
    pub bases: Vec<String>,
    pub parent_class: Option<String>,
    pub statement_kind: Option<StatementKind>,  // Populated when kind == Statement
    pub function_idx: Option<NodeIndex>,        // Owning function (for Statement/CfgEntry/CfgExit)
}

/// Edge types within the CPG layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpgEdge {
    /// File → Function/Class, Class → Method
    AstChild,
    /// Sequential control flow
    ControlFlowNext,
    /// Conditional true branch
    ControlFlowTrue,
    /// Conditional false branch
    ControlFlowFalse,
    /// Exception handler edge
    ControlFlowException,
    /// Loop back-edge (continue, end of loop body)
    ControlFlowBack,
    /// Data flow: definition at source reaches use at target (variable name)
    DataFlowReach(String),
    /// Function A calls Function B (between Function/Method nodes)
    Calls,
    /// Reverse of Calls: Function B is called by Function A
    CalledBy,
    /// Argument flows from call-site statement to callee CfgEntry (positional)
    DataFlowArgument { position: usize },
    /// Return value flows from callee return statement to caller call-site statement
    DataFlowReturn,
}

/// The CPG overlay layer. Owns its own graph, separate from the file-level RepoGraph.
pub struct CpgLayer {
    pub graph: DiGraph<CpgNode, CpgEdge>,
    pub file_to_nodes: HashMap<PathBuf, Vec<NodeIndex>>,
    pub trees: HashMap<PathBuf, Tree>,
    pub sources: HashMap<PathBuf, String>,
    pub function_to_entry: HashMap<NodeIndex, NodeIndex>,  // Function → CfgEntry
    pub function_to_exit: HashMap<NodeIndex, NodeIndex>,   // Function → CfgExit
    pub stmt_defs: HashMap<NodeIndex, Vec<String>>,        // Statement → defined variables
    pub stmt_uses: HashMap<NodeIndex, Vec<String>>,        // Statement → used variables
    pub call_sites: HashMap<NodeIndex, Vec<crate::callgraph::CallSite>>,  // func_idx → call sites
    pub name_to_funcs: HashMap<String, Vec<NodeIndex>>,   // func name → defining node indices
    pub function_to_stmts: HashMap<NodeIndex, Vec<NodeIndex>>,  // func_idx → owned Statement/CfgEntry/CfgExit nodes
    pub import_bindings: HashMap<PathBuf, Vec<ImportBinding>>,  // file → import bindings for call resolution
}

impl std::fmt::Debug for CpgLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CpgLayer")
            .field("node_count", &self.graph.node_count())
            .field("edge_count", &self.graph.edge_count())
            .field("files_tracked", &self.file_to_nodes.len())
            .finish()
    }
}

impl CpgLayer {
    /// Create an empty CPG layer.
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            file_to_nodes: HashMap::new(),
            trees: HashMap::new(),
            sources: HashMap::new(),
            function_to_entry: HashMap::new(),
            function_to_exit: HashMap::new(),
            stmt_defs: HashMap::new(),
            stmt_uses: HashMap::new(),
            call_sites: HashMap::new(),
            name_to_funcs: HashMap::new(),
            function_to_stmts: HashMap::new(),
            import_bindings: HashMap::new(),
        }
    }

    /// Get all Statement/CfgEntry/CfgExit nodes owned by a function.
    pub fn get_stmts_for_function(&self, func_idx: NodeIndex) -> &[NodeIndex] {
        self.function_to_stmts
            .get(&func_idx)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Check if CPG data has been built for a given file.
    pub fn has_file(&self, path: &Path) -> bool {
        self.file_to_nodes.contains_key(path)
    }

    /// Extract CPG nodes from an AST and store the tree + source.
    pub fn build_file(
        &mut self,
        path: &Path,
        tree: Tree,
        source: String,
        language: SupportedLanguage,
    ) {
        let nodes = match language {
            SupportedLanguage::Python => extract_python_nodes(path, &tree, &source),
            SupportedLanguage::JavaScript
            | SupportedLanguage::JavaScriptJsx
            | SupportedLanguage::TypeScript
            | SupportedLanguage::TypeScriptTsx => extract_js_nodes(path, &tree, &source),
            _ => Vec::new(),
        };

        let mut node_indices = Vec::new();

        for extracted in nodes {
            match extracted {
                ExtractedItem::TopLevel(cpg_node) => {
                    let idx = self.graph.add_node(cpg_node);
                    node_indices.push(idx);
                }
                ExtractedItem::ClassWithMethods {
                    class_node,
                    methods,
                } => {
                    let class_idx = self.graph.add_node(class_node);
                    node_indices.push(class_idx);

                    for method_node in methods {
                        let method_idx = self.graph.add_node(method_node);
                        node_indices.push(method_idx);
                        self.graph.add_edge(class_idx, method_idx, CpgEdge::AstChild);
                    }
                }
            }
        }

        self.file_to_nodes.insert(path.to_path_buf(), node_indices.clone());
        self.trees.insert(path.to_path_buf(), tree);
        self.sources.insert(path.to_path_buf(), source);

        // Build CFG, dataflow, and call sites for languages that produced nodes
        if !node_indices.is_empty() {
            let func_count = node_indices.iter()
                .filter(|idx| matches!(self.graph[**idx].kind, CpgNodeKind::Function | CpgNodeKind::Method))
                .count();

            let phase_start = Instant::now();
            let cfg_builder = crate::cfg::CfgBuilder::new(language);
            // Clone source to avoid borrow conflict (self.sources borrowed vs &mut self)
            let source_clone = self.sources.get(&path.to_path_buf()).unwrap().clone();
            for &idx in &node_indices {
                if matches!(self.graph[idx].kind, CpgNodeKind::Function | CpgNodeKind::Method) {
                    let name = self.graph[idx].name.clone();
                    crate::diag::diag_log(&format!("[DIAG]   CFG: {}()", name));
                    let func_start = Instant::now();
                    let _ = cfg_builder.build_function_cfg(self, idx, &source_clone);
                    let elapsed = func_start.elapsed();
                    if elapsed.as_millis() > 100 {
                        crate::diag::diag_log(&format!("[DIAG]   slow CFG: {}() {:.2}s", name, elapsed.as_secs_f64()));
                    }
                }
            }
            let cfg_elapsed = phase_start.elapsed();

            // Data flow analysis: reaching definitions for each function
            let phase_start = Instant::now();
            let source_clone = self.sources.get(&path.to_path_buf()).unwrap().clone();
            for &idx in &node_indices {
                if matches!(self.graph[idx].kind, CpgNodeKind::Function | CpgNodeKind::Method) {
                    let name = self.graph[idx].name.clone();
                    crate::diag::diag_log(&format!("[DIAG]   dataflow: {}()", name));
                    let func_start = Instant::now();
                    crate::dataflow::DataFlowAnalyzer::analyze_function(self, idx, &source_clone);
                    let elapsed = func_start.elapsed();
                    if elapsed.as_millis() > 100 {
                        crate::diag::diag_log(&format!("[DIAG]   slow dataflow: {}() {:.2}s", name, elapsed.as_secs_f64()));
                    }
                }
            }
            let dataflow_elapsed = phase_start.elapsed();

            // Call site extraction (Pass 1): extract raw call sites per function
            let phase_start = Instant::now();
            let source_clone = self.sources.get(&path.to_path_buf()).unwrap().clone();
            for &idx in &node_indices {
                if matches!(self.graph[idx].kind, CpgNodeKind::Function | CpgNodeKind::Method) {
                    let sites = crate::callgraph::CallGraphBuilder::extract_call_sites(self, idx, &source_clone);
                    self.call_sites.insert(idx, sites);
                }
            }
            let callsite_elapsed = phase_start.elapsed();

            // Log per-file phase timing if any phase took >1s
            let total_ms = cfg_elapsed.as_millis() + dataflow_elapsed.as_millis() + callsite_elapsed.as_millis();
            if total_ms > 200 {
                crate::diag::diag_log(&format!("[DIAG] build_file slow ({} funcs): cfg={:.2}s dataflow={:.2}s callsites={:.2}s file={}",
                    func_count,
                    cfg_elapsed.as_secs_f64(),
                    dataflow_elapsed.as_secs_f64(),
                    callsite_elapsed.as_secs_f64(),
                    path.display()));
            }

            // Update name_to_funcs index for this file
            self.add_to_name_index(path);
        }
    }

    /// Remove all CPG nodes for a file.
    pub fn remove_file(&mut self, path: &Path) {
        // Remove import bindings for this file
        self.import_bindings.remove(path);
        // Remove from name_to_funcs before removing nodes
        self.remove_from_name_index(path);

        if let Some(indices) = self.file_to_nodes.remove(path) {
            // Also collect function_to_entry/exit entries to remove
            let func_indices: Vec<NodeIndex> = indices.iter()
                .filter(|idx| {
                    self.graph.node_weight(**idx)
                        .map(|n| matches!(n.kind, CpgNodeKind::Function | CpgNodeKind::Method))
                        .unwrap_or(false)
                })
                .copied()
                .collect();
            for fi in &func_indices {
                self.function_to_entry.remove(fi);
                self.function_to_exit.remove(fi);
                self.call_sites.remove(fi);
                self.function_to_stmts.remove(fi);
            }

            // Clean up stmt_defs/stmt_uses for all nodes being removed
            for idx in &indices {
                self.stmt_defs.remove(idx);
                self.stmt_uses.remove(idx);
            }

            // Remove in descending order to avoid index invalidation from swap-remove
            let mut sorted = indices;
            sorted.sort_by(|a, b| b.index().cmp(&a.index()));

            for idx in sorted {
                // Before removing, update file_to_nodes for the node that will be swapped in
                let last_idx = NodeIndex::new(self.graph.node_count() - 1);
                if idx != last_idx {
                    // The node at last_idx is about to be moved to idx
                    if let Some(moved_node) = self.graph.node_weight(last_idx) {
                        let moved_path = moved_node.file_path.clone();
                        // Update the reference in file_to_nodes
                        if let Some(node_list) = self.file_to_nodes.get_mut(&moved_path) {
                            for entry in node_list.iter_mut() {
                                if *entry == last_idx {
                                    *entry = idx;
                                }
                            }
                        }
                    }

                    // Also remap function_to_entry/function_to_exit for swapped node
                    // Check if last_idx is a key in function_to_entry
                    if let Some(entry_idx) = self.function_to_entry.remove(&last_idx) {
                        self.function_to_entry.insert(idx, entry_idx);
                    }
                    if let Some(exit_idx) = self.function_to_exit.remove(&last_idx) {
                        self.function_to_exit.insert(idx, exit_idx);
                    }
                    // Check if last_idx is a value in the maps (entry/exit nodes being swapped)
                    for val in self.function_to_entry.values_mut() {
                        if *val == last_idx {
                            *val = idx;
                        }
                    }
                    for val in self.function_to_exit.values_mut() {
                        if *val == last_idx {
                            *val = idx;
                        }
                    }

                    // Remap stmt_defs/stmt_uses for swapped node
                    if let Some(defs) = self.stmt_defs.remove(&last_idx) {
                        self.stmt_defs.insert(idx, defs);
                    }
                    if let Some(uses) = self.stmt_uses.remove(&last_idx) {
                        self.stmt_uses.insert(idx, uses);
                    }

                    // Remap call_sites for swapped node
                    if let Some(sites) = self.call_sites.remove(&last_idx) {
                        // Update internal NodeIndex fields in each call site
                        let sites: Vec<_> = sites.into_iter().map(|mut s| {
                            if s.stmt_idx == last_idx { s.stmt_idx = idx; }
                            if s.caller_func_idx == last_idx { s.caller_func_idx = idx; }
                            s
                        }).collect();
                        self.call_sites.insert(idx, sites);
                    }
                    // Also update call_sites values that reference last_idx in other funcs
                    for sites in self.call_sites.values_mut() {
                        for site in sites.iter_mut() {
                            if site.stmt_idx == last_idx { site.stmt_idx = idx; }
                            if site.caller_func_idx == last_idx { site.caller_func_idx = idx; }
                        }
                    }

                    // Remap name_to_funcs values
                    for func_indices in self.name_to_funcs.values_mut() {
                        for fi in func_indices.iter_mut() {
                            if *fi == last_idx {
                                *fi = idx;
                            }
                        }
                    }

                    // Remap function_to_stmts: key (if last_idx was a function)
                    if let Some(stmts) = self.function_to_stmts.remove(&last_idx) {
                        let stmts: Vec<NodeIndex> = stmts.into_iter().map(|s| {
                            if s == last_idx { idx } else { s }
                        }).collect();
                        self.function_to_stmts.insert(idx, stmts);
                    }
                    // Remap function_to_stmts: values that reference last_idx
                    for stmt_list in self.function_to_stmts.values_mut() {
                        for s in stmt_list.iter_mut() {
                            if *s == last_idx {
                                *s = idx;
                            }
                        }
                    }
                }
                self.graph.remove_node(idx);
            }
        }
        self.trees.remove(path);
        self.sources.remove(path);
    }

    /// Update a file's CPG data: removes old nodes, then rebuilds.
    pub fn update_file(
        &mut self,
        path: &Path,
        tree: Tree,
        source: String,
        language: SupportedLanguage,
    ) {
        self.remove_file(path);
        self.build_file(path, tree, source, language);
    }

    /// Get all CPG nodes for a file.
    pub fn get_nodes_for_file(&self, path: &Path) -> Vec<&CpgNode> {
        self.file_to_nodes
            .get(path)
            .map(|indices| {
                indices
                    .iter()
                    .filter_map(|idx| self.graph.node_weight(*idx))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get function, method, and class nodes for a file.
    pub fn get_functions_in_file(&self, path: &Path) -> Vec<&CpgNode> {
        self.get_nodes_for_file(path)
            .into_iter()
            .filter(|n| matches!(n.kind, CpgNodeKind::Function | CpgNodeKind::Method | CpgNodeKind::Class))
            .collect()
    }

    /// Get class nodes for a file.
    pub fn get_classes_in_file(&self, path: &Path) -> Vec<&CpgNode> {
        self.get_nodes_for_file(path)
            .into_iter()
            .filter(|n| n.kind == CpgNodeKind::Class)
            .collect()
    }

    /// Get children of a node (follows AstChild edges).
    pub fn get_children(&self, node_idx: NodeIndex) -> Vec<(NodeIndex, &CpgNode)> {
        self.graph
            .edges_directed(node_idx, petgraph::Direction::Outgoing)
            .filter(|e| *e.weight() == CpgEdge::AstChild)
            .filter_map(|e| {
                let target = e.target();
                self.graph.node_weight(target).map(|n| (target, n))
            })
            .collect()
    }

    /// Get the persisted AST for a file.
    pub fn get_tree(&self, path: &Path) -> Option<&Tree> {
        self.trees.get(path)
    }

    /// Get the persisted source for a file.
    pub fn get_source(&self, path: &Path) -> Option<&str> {
        self.sources.get(path).map(|s| s.as_str())
    }

    /// Get all CFG edges within a function's scope.
    pub fn get_cfg_edges_for_function(&self, func_idx: NodeIndex) -> Vec<(NodeIndex, NodeIndex, &CpgEdge)> {
        let mut result = Vec::new();

        let func_nodes: HashSet<NodeIndex> = self.get_stmts_for_function(func_idx)
            .iter()
            .copied()
            .collect();

        // Collect all CFG edges between these nodes
        for &node_idx in &func_nodes {
            for edge in self.graph.edges_directed(node_idx, petgraph::Direction::Outgoing) {
                if matches!(edge.weight(),
                    CpgEdge::ControlFlowNext
                    | CpgEdge::ControlFlowTrue
                    | CpgEdge::ControlFlowFalse
                    | CpgEdge::ControlFlowException
                    | CpgEdge::ControlFlowBack
                ) {
                    result.push((edge.source(), edge.target(), edge.weight()));
                }
            }
        }

        result
    }

    /// Add functions from a file to the name_to_funcs index.
    pub fn add_to_name_index(&mut self, path: &Path) {
        if let Some(indices) = self.file_to_nodes.get(path) {
            for &idx in indices {
                if let Some(node) = self.graph.node_weight(idx) {
                    if matches!(node.kind, CpgNodeKind::Function | CpgNodeKind::Method) {
                        self.name_to_funcs
                            .entry(node.name.clone())
                            .or_default()
                            .push(idx);
                    }
                }
            }
        }
    }

    /// Remove functions of a file from the name_to_funcs index.
    pub fn remove_from_name_index(&mut self, path: &Path) {
        if let Some(indices) = self.file_to_nodes.get(path) {
            let idx_set: HashSet<NodeIndex> = indices.iter().copied().collect();
            self.name_to_funcs.retain(|_, func_indices| {
                func_indices.retain(|fi| !idx_set.contains(fi));
                !func_indices.is_empty()
            });
        }
    }

    /// Rebuild the entire name_to_funcs index from scratch.
    pub fn rebuild_name_index(&mut self) {
        self.name_to_funcs.clear();
        for idx in self.graph.node_indices() {
            if let Some(node) = self.graph.node_weight(idx) {
                if matches!(node.kind, CpgNodeKind::Function | CpgNodeKind::Method) {
                    self.name_to_funcs
                        .entry(node.name.clone())
                        .or_default()
                        .push(idx);
                }
            }
        }
    }

    /// Get all functions called by a given function (follows Calls edges).
    pub fn get_callees(&self, func_idx: NodeIndex) -> Vec<(NodeIndex, &CpgNode)> {
        let mut seen = HashSet::new();
        self.graph
            .edges_directed(func_idx, petgraph::Direction::Outgoing)
            .filter(|e| *e.weight() == CpgEdge::Calls)
            .filter_map(|e| {
                let target = e.target();
                if seen.insert(target) {
                    self.graph.node_weight(target).map(|n| (target, n))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get all functions that call a given function (follows CalledBy edges).
    pub fn get_callers(&self, func_idx: NodeIndex) -> Vec<(NodeIndex, &CpgNode)> {
        let mut seen = HashSet::new();
        self.graph
            .edges_directed(func_idx, petgraph::Direction::Outgoing)
            .filter(|e| *e.weight() == CpgEdge::CalledBy)
            .filter_map(|e| {
                let target = e.target();
                if seen.insert(target) {
                    self.graph.node_weight(target).map(|n| (target, n))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get all DataFlowReach edges within a function's scope.
    /// Returns (def_node, use_node, variable_name) tuples.
    pub fn get_dataflow_edges_for_function(&self, func_idx: NodeIndex) -> Vec<(NodeIndex, NodeIndex, &str)> {
        let mut result = Vec::new();

        let func_nodes: HashSet<NodeIndex> = self.get_stmts_for_function(func_idx)
            .iter()
            .copied()
            .collect();

        // Collect all DataFlowReach edges between these nodes
        for &node_idx in &func_nodes {
            for edge in self.graph.edges_directed(node_idx, petgraph::Direction::Outgoing) {
                if let CpgEdge::DataFlowReach(var_name) = edge.weight() {
                    result.push((edge.source(), edge.target(), var_name.as_str()));
                }
            }
        }

        result
    }
}

// ---------------------------------------------------------------------------
// Python extraction
// ---------------------------------------------------------------------------

/// Internal type to represent extraction results before graph insertion.
enum ExtractedItem {
    TopLevel(CpgNode),
    ClassWithMethods {
        class_node: CpgNode,
        methods: Vec<CpgNode>,
    },
}

fn extract_python_nodes(
    file_path: &Path,
    tree: &Tree,
    source: &str,
) -> Vec<ExtractedItem> {
    let mut items = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        match child.kind() {
            "function_definition" | "async_function_definition" => {
                if let Some(node) = extract_python_function(file_path, child, source, None) {
                    items.push(ExtractedItem::TopLevel(node));
                }
            }
            "class_definition" => {
                if let Some(item) = extract_python_class(file_path, child, source) {
                    items.push(item);
                }
            }
            "decorated_definition" => {
                extract_decorated(file_path, child, source, &mut items);
            }
            "expression_statement" => {
                if let Some(node) = extract_module_variable(file_path, child, source) {
                    items.push(ExtractedItem::TopLevel(node));
                }
            }
            _ => {}
        }
    }

    items
}

/// Extract a Python function or method node from a tree-sitter node.
fn extract_python_function(
    file_path: &Path,
    node: tree_sitter::Node,
    source: &str,
    parent_class: Option<&str>,
) -> Option<CpgNode> {
    let name_node = node.child_by_field_name("name")?;
    let name = name_node.utf8_text(source.as_bytes()).ok()?.to_string();

    let kind = if parent_class.is_some() {
        CpgNodeKind::Method
    } else {
        CpgNodeKind::Function
    };

    let parameters = extract_python_parameters(node, source);

    let return_type = node
        .child_by_field_name("return_type")
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
        .map(|s| s.to_string());

    let docstring = extract_python_docstring(node, source);

    Some(CpgNode {
        file_path: file_path.to_path_buf(),
        name,
        kind,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        parameters,
        return_type,
        docstring,
        bases: Vec::new(),
        parent_class: parent_class.map(|s| s.to_string()),
        statement_kind: None,
        function_idx: None,
    })
}

/// Extract a Python class and its methods.
fn extract_python_class(
    file_path: &Path,
    node: tree_sitter::Node,
    source: &str,
) -> Option<ExtractedItem> {
    let name_node = node.child_by_field_name("name")?;
    let class_name = name_node.utf8_text(source.as_bytes()).ok()?.to_string();

    let bases = extract_python_bases(node, source);
    let docstring = extract_python_class_docstring(node, source);

    let class_node = CpgNode {
        file_path: file_path.to_path_buf(),
        name: class_name.clone(),
        kind: CpgNodeKind::Class,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        parameters: Vec::new(),
        return_type: None,
        docstring,
        bases,
        parent_class: None,
        statement_kind: None,
        function_idx: None,
    };

    let mut methods = Vec::new();

    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            match child.kind() {
                "function_definition" | "async_function_definition" => {
                    if let Some(method) =
                        extract_python_function(file_path, child, source, Some(&class_name))
                    {
                        methods.push(method);
                    }
                }
                "decorated_definition" => {
                    extract_decorated_methods(
                        file_path,
                        child,
                        source,
                        &class_name,
                        &mut methods,
                    );
                }
                _ => {}
            }
        }
    }

    Some(ExtractedItem::ClassWithMethods {
        class_node,
        methods,
    })
}

/// Extract parameters from a Python function node.
fn extract_python_parameters(
    func_node: tree_sitter::Node,
    source: &str,
) -> Vec<Parameter> {
    let params_node = match func_node.child_by_field_name("parameters") {
        Some(n) => n,
        None => return Vec::new(),
    };

    let mut parameters = Vec::new();
    let mut cursor = params_node.walk();

    for child in params_node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                // Simple parameter: `a`
                if let Ok(name) = child.utf8_text(source.as_bytes()) {
                    parameters.push(Parameter {
                        name: name.to_string(),
                        type_annotation: None,
                        default_value: None,
                    });
                }
            }
            "typed_parameter" => {
                // `a: int`
                let name = child
                    .named_child(0)
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .unwrap_or("")
                    .to_string();
                let type_ann = child
                    .child_by_field_name("type")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string());
                parameters.push(Parameter {
                    name,
                    type_annotation: type_ann,
                    default_value: None,
                });
            }
            "default_parameter" => {
                // `a=5`
                let name = child
                    .child_by_field_name("name")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .unwrap_or("")
                    .to_string();
                let default = child
                    .child_by_field_name("value")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string());
                parameters.push(Parameter {
                    name,
                    type_annotation: None,
                    default_value: default,
                });
            }
            "typed_default_parameter" => {
                // `a: int = 5`
                let name = child
                    .child_by_field_name("name")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .unwrap_or("")
                    .to_string();
                let type_ann = child
                    .child_by_field_name("type")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string());
                let default = child
                    .child_by_field_name("value")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string());
                parameters.push(Parameter {
                    name,
                    type_annotation: type_ann,
                    default_value: default,
                });
            }
            "list_splat_pattern" | "dictionary_splat_pattern" => {
                // `*args` or `**kwargs`
                if let Ok(text) = child.utf8_text(source.as_bytes()) {
                    parameters.push(Parameter {
                        name: text.to_string(),
                        type_annotation: None,
                        default_value: None,
                    });
                }
            }
            _ => {}
        }
    }

    parameters
}

/// Extract base classes from a Python class node.
fn extract_python_bases(
    class_node: tree_sitter::Node,
    source: &str,
) -> Vec<String> {
    let superclasses = match class_node.child_by_field_name("superclasses") {
        Some(n) => n,
        None => return Vec::new(),
    };

    let mut bases = Vec::new();
    let mut cursor = superclasses.walk();

    for child in superclasses.children(&mut cursor) {
        if child.is_named() {
            if let Ok(text) = child.utf8_text(source.as_bytes()) {
                bases.push(text.to_string());
            }
        }
    }

    bases
}

/// Extract the docstring from a function body.
fn extract_python_docstring(
    func_node: tree_sitter::Node,
    source: &str,
) -> Option<String> {
    let body = func_node.child_by_field_name("body")?;
    let first_stmt = body.named_child(0)?;

    if first_stmt.kind() != "expression_statement" {
        return None;
    }

    let expr = first_stmt.named_child(0)?;
    if expr.kind() != "string" && expr.kind() != "concatenated_string" {
        return None;
    }

    expr.utf8_text(source.as_bytes()).ok().map(|s| s.to_string())
}

/// Extract the docstring from a class body.
fn extract_python_class_docstring(
    class_node: tree_sitter::Node,
    source: &str,
) -> Option<String> {
    let body = class_node.child_by_field_name("body")?;
    let first_stmt = body.named_child(0)?;

    if first_stmt.kind() != "expression_statement" {
        return None;
    }

    let expr = first_stmt.named_child(0)?;
    if expr.kind() != "string" && expr.kind() != "concatenated_string" {
        return None;
    }

    expr.utf8_text(source.as_bytes()).ok().map(|s| s.to_string())
}

/// Handle a decorated definition at module level.
fn extract_decorated(
    file_path: &Path,
    node: tree_sitter::Node,
    source: &str,
    items: &mut Vec<ExtractedItem>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" | "async_function_definition" => {
                if let Some(cpg_node) = extract_python_function(file_path, child, source, None) {
                    items.push(ExtractedItem::TopLevel(cpg_node));
                }
            }
            "class_definition" => {
                if let Some(item) = extract_python_class(file_path, child, source) {
                    items.push(item);
                }
            }
            _ => {}
        }
    }
}

/// Handle a decorated definition inside a class body.
fn extract_decorated_methods(
    file_path: &Path,
    node: tree_sitter::Node,
    source: &str,
    class_name: &str,
    methods: &mut Vec<CpgNode>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" | "async_function_definition" => {
                if let Some(method) =
                    extract_python_function(file_path, child, source, Some(class_name))
                {
                    methods.push(method);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// JS/TS extraction
// ---------------------------------------------------------------------------

fn extract_js_nodes(
    file_path: &Path,
    tree: &Tree,
    source: &str,
) -> Vec<ExtractedItem> {
    let mut items = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        extract_js_top_level(file_path, child, source, &mut items);
    }

    items
}

/// Process a single top-level JS/TS node into ExtractedItems.
fn extract_js_top_level(
    file_path: &Path,
    node: tree_sitter::Node,
    source: &str,
    items: &mut Vec<ExtractedItem>,
) {
    match node.kind() {
        "function_declaration" | "generator_function_declaration" => {
            if let Some(cpg_node) = extract_js_function(file_path, node, source, None) {
                items.push(ExtractedItem::TopLevel(cpg_node));
            }
        }
        "class_declaration" => {
            if let Some(item) = extract_js_class(file_path, node, source) {
                items.push(item);
            }
        }
        "export_statement" => {
            // Unwrap to inner declaration
            let mut inner_cursor = node.walk();
            for child in node.named_children(&mut inner_cursor) {
                match child.kind() {
                    "function_declaration" | "generator_function_declaration" => {
                        if let Some(cpg_node) = extract_js_function(file_path, child, source, None) {
                            items.push(ExtractedItem::TopLevel(cpg_node));
                        }
                    }
                    "class_declaration" => {
                        if let Some(item) = extract_js_class(file_path, child, source) {
                            items.push(item);
                        }
                    }
                    "lexical_declaration" | "variable_declaration" => {
                        extract_js_variable_declaration(file_path, child, source, items);
                    }
                    _ => {}
                }
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            extract_js_variable_declaration(file_path, node, source, items);
        }
        "expression_statement" => {
            // Handle module.exports = ... or similar assignments
            // Skip for now — not a common pattern for function/class extraction
        }
        _ => {}
    }
}

/// Extract variable declarations (const/let/var), recognizing arrow functions.
fn extract_js_variable_declaration(
    file_path: &Path,
    node: tree_sitter::Node,
    source: &str,
    items: &mut Vec<ExtractedItem>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            let name_node = match child.child_by_field_name("name") {
                Some(n) if n.kind() == "identifier" => n,
                _ => continue,
            };
            let name = match name_node.utf8_text(source.as_bytes()) {
                Ok(s) => s.to_string(),
                Err(_) => continue,
            };

            let value = child.child_by_field_name("value");
            let is_function = value
                .map(|v| matches!(v.kind(), "arrow_function" | "function" | "function_expression" | "generator_function"))
                .unwrap_or(false);

            if is_function {
                let value_node = value.unwrap();
                let parameters = extract_js_parameters(value_node, source);
                let return_type = extract_js_return_type(value_node, source);
                let docstring = extract_js_docstring(node, source);

                items.push(ExtractedItem::TopLevel(CpgNode {
                    file_path: file_path.to_path_buf(),
                    name,
                    kind: CpgNodeKind::Function,
                    start_byte: node.start_byte(),
                    end_byte: node.end_byte(),
                    start_line: node.start_position().row + 1,
                    end_line: node.end_position().row + 1,
                    parameters,
                    return_type,
                    docstring,
                    bases: Vec::new(),
                    parent_class: None,
                    statement_kind: None,
                    function_idx: None,
                }));
            } else {
                items.push(ExtractedItem::TopLevel(CpgNode {
                    file_path: file_path.to_path_buf(),
                    name,
                    kind: CpgNodeKind::Variable,
                    start_byte: node.start_byte(),
                    end_byte: node.end_byte(),
                    start_line: node.start_position().row + 1,
                    end_line: node.end_position().row + 1,
                    parameters: Vec::new(),
                    return_type: None,
                    docstring: None,
                    bases: Vec::new(),
                    parent_class: None,
                    statement_kind: None,
                    function_idx: None,
                }));
            }
        }
    }
}

/// Extract a JS/TS function or method node.
fn extract_js_function(
    file_path: &Path,
    node: tree_sitter::Node,
    source: &str,
    parent_class: Option<&str>,
) -> Option<CpgNode> {
    let name_node = node.child_by_field_name("name")?;
    let name = name_node.utf8_text(source.as_bytes()).ok()?.to_string();

    let kind = if parent_class.is_some() {
        CpgNodeKind::Method
    } else {
        CpgNodeKind::Function
    };

    let parameters = extract_js_parameters(node, source);
    let return_type = extract_js_return_type(node, source);
    let docstring = extract_js_docstring(node, source);

    Some(CpgNode {
        file_path: file_path.to_path_buf(),
        name,
        kind,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        parameters,
        return_type,
        docstring,
        bases: Vec::new(),
        parent_class: parent_class.map(|s| s.to_string()),
        statement_kind: None,
        function_idx: None,
    })
}

/// Extract a JS/TS class and its methods.
fn extract_js_class(
    file_path: &Path,
    node: tree_sitter::Node,
    source: &str,
) -> Option<ExtractedItem> {
    let name_node = node.child_by_field_name("name")?;
    let class_name = name_node.utf8_text(source.as_bytes()).ok()?.to_string();

    // Heritage clause for bases (extends/implements)
    let bases = extract_js_heritage(node, source);
    let docstring = extract_js_docstring(node, source);

    let class_node = CpgNode {
        file_path: file_path.to_path_buf(),
        name: class_name.clone(),
        kind: CpgNodeKind::Class,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        parameters: Vec::new(),
        return_type: None,
        docstring,
        bases,
        parent_class: None,
        statement_kind: None,
        function_idx: None,
    };

    let mut methods = Vec::new();

    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            match child.kind() {
                "method_definition" => {
                    if let Some(method) = extract_js_method(file_path, child, source, &class_name) {
                        methods.push(method);
                    }
                }
                "public_field_definition" | "property_definition" => {
                    // Could be an arrow function property — check value
                    if let Some(name_n) = child.child_by_field_name("name") {
                        if let Ok(prop_name) = name_n.utf8_text(source.as_bytes()) {
                            let value = child.child_by_field_name("value");
                            let is_fn = value
                                .map(|v| matches!(v.kind(), "arrow_function" | "function" | "function_expression"))
                                .unwrap_or(false);
                            if is_fn {
                                let value_node = value.unwrap();
                                let parameters = extract_js_parameters(value_node, source);
                                let return_type = extract_js_return_type(value_node, source);
                                methods.push(CpgNode {
                                    file_path: file_path.to_path_buf(),
                                    name: prop_name.to_string(),
                                    kind: CpgNodeKind::Method,
                                    start_byte: child.start_byte(),
                                    end_byte: child.end_byte(),
                                    start_line: child.start_position().row + 1,
                                    end_line: child.end_position().row + 1,
                                    parameters,
                                    return_type,
                                    docstring: None,
                                    bases: Vec::new(),
                                    parent_class: Some(class_name.clone()),
                                    statement_kind: None,
                                    function_idx: None,
                                });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    Some(ExtractedItem::ClassWithMethods {
        class_node,
        methods,
    })
}

/// Extract a method_definition node from a class body.
fn extract_js_method(
    file_path: &Path,
    node: tree_sitter::Node,
    source: &str,
    class_name: &str,
) -> Option<CpgNode> {
    let name_node = node.child_by_field_name("name")?;
    let name = name_node.utf8_text(source.as_bytes()).ok()?.to_string();

    let parameters = extract_js_parameters(node, source);
    let return_type = extract_js_return_type(node, source);
    let docstring = extract_js_docstring(node, source);

    Some(CpgNode {
        file_path: file_path.to_path_buf(),
        name,
        kind: CpgNodeKind::Method,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        parameters,
        return_type,
        docstring,
        bases: Vec::new(),
        parent_class: Some(class_name.to_string()),
        statement_kind: None,
        function_idx: None,
    })
}

/// Extract parameters from a JS/TS function node.
fn extract_js_parameters(
    func_node: tree_sitter::Node,
    source: &str,
) -> Vec<Parameter> {
    let params_node = match func_node.child_by_field_name("parameters") {
        Some(n) => n,
        None => return Vec::new(),
    };

    let mut parameters = Vec::new();
    let mut cursor = params_node.walk();

    for child in params_node.named_children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                if let Ok(name) = child.utf8_text(source.as_bytes()) {
                    parameters.push(Parameter {
                        name: name.to_string(),
                        type_annotation: None,
                        default_value: None,
                    });
                }
            }
            "required_parameter" => {
                // TS: `name: type`
                let name = child
                    .child_by_field_name("pattern")
                    .or_else(|| child.named_child(0))
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .unwrap_or("")
                    .to_string();
                let type_ann = child
                    .child_by_field_name("type")
                    .and_then(|n| {
                        // The type_annotation node wraps the actual type
                        if n.kind() == "type_annotation" {
                            n.named_child(0)
                        } else {
                            Some(n)
                        }
                    })
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string());
                parameters.push(Parameter {
                    name,
                    type_annotation: type_ann,
                    default_value: None,
                });
            }
            "optional_parameter" => {
                // TS: `name?: type` or `name: type = default`
                let name = child
                    .child_by_field_name("pattern")
                    .or_else(|| child.named_child(0))
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .unwrap_or("")
                    .to_string();
                let type_ann = child
                    .child_by_field_name("type")
                    .and_then(|n| {
                        if n.kind() == "type_annotation" {
                            n.named_child(0)
                        } else {
                            Some(n)
                        }
                    })
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string());
                let default = child
                    .child_by_field_name("value")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string());
                parameters.push(Parameter {
                    name,
                    type_annotation: type_ann,
                    default_value: default,
                });
            }
            "assignment_pattern" => {
                // `name = default`
                let name = child
                    .child_by_field_name("left")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .unwrap_or("")
                    .to_string();
                let default = child
                    .child_by_field_name("right")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string());
                parameters.push(Parameter {
                    name,
                    type_annotation: None,
                    default_value: default,
                });
            }
            "rest_pattern" | "rest_parameter" => {
                // `...args`
                if let Ok(text) = child.utf8_text(source.as_bytes()) {
                    parameters.push(Parameter {
                        name: text.to_string(),
                        type_annotation: None,
                        default_value: None,
                    });
                }
            }
            _ => {}
        }
    }

    parameters
}

/// Extract return type from a JS/TS function (TS type_annotation on return).
fn extract_js_return_type(
    func_node: tree_sitter::Node,
    source: &str,
) -> Option<String> {
    // TS uses "return_type" field
    func_node
        .child_by_field_name("return_type")
        .and_then(|n| {
            // The return_type is a type_annotation node; get the inner type
            if n.kind() == "type_annotation" {
                n.named_child(0)
            } else {
                Some(n)
            }
        })
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
        .map(|s| s.to_string())
}

/// Extract JSDoc comment (/** ... */) preceding a node.
fn extract_js_docstring(
    node: tree_sitter::Node,
    source: &str,
) -> Option<String> {
    // Look for a preceding comment sibling starting with /**
    let mut prev = node.prev_named_sibling();
    while let Some(sibling) = prev {
        if sibling.kind() == "comment" {
            let text = sibling.utf8_text(source.as_bytes()).ok()?;
            if text.starts_with("/**") {
                return Some(text.to_string());
            }
            return None;
        }
        // Skip over decorators/export keywords
        if sibling.kind() != "decorator" {
            return None;
        }
        prev = sibling.prev_named_sibling();
    }
    None
}

/// Extract heritage (extends/implements) from a JS/TS class.
fn extract_js_heritage(
    class_node: tree_sitter::Node,
    source: &str,
) -> Vec<String> {
    let mut bases = Vec::new();

    // JS tree-sitter: class_heritage is a direct child containing identifiers
    let mut cursor = class_node.walk();
    for child in class_node.named_children(&mut cursor) {
        if child.kind() == "class_heritage" {
            // Collect all named children (identifiers, member_expressions)
            let mut inner = child.walk();
            for heritage_child in child.named_children(&mut inner) {
                match heritage_child.kind() {
                    "extends_clause" | "implements_clause" => {
                        let mut ext_cursor = heritage_child.walk();
                        for ext_child in heritage_child.named_children(&mut ext_cursor) {
                            if let Ok(text) = ext_child.utf8_text(source.as_bytes()) {
                                bases.push(text.to_string());
                            }
                        }
                    }
                    "identifier" | "member_expression" => {
                        // Direct identifier in class_heritage (JS)
                        if let Ok(text) = heritage_child.utf8_text(source.as_bytes()) {
                            bases.push(text.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    bases
}

/// Extract a module-level variable assignment.
fn extract_module_variable(
    file_path: &Path,
    node: tree_sitter::Node,
    source: &str,
) -> Option<CpgNode> {
    // expression_statement → assignment
    let expr = node.named_child(0)?;

    if expr.kind() != "assignment" {
        return None;
    }

    // Get the left-hand side (the variable name)
    let lhs = expr.child_by_field_name("left")?;

    // Only handle simple identifier assignments (not tuple unpacking, etc.)
    if lhs.kind() != "identifier" {
        return None;
    }

    let name = lhs.utf8_text(source.as_bytes()).ok()?.to_string();

    Some(CpgNode {
        file_path: file_path.to_path_buf(),
        name,
        kind: CpgNodeKind::Variable,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        parameters: Vec::new(),
        return_type: None,
        docstring: None,
        bases: Vec::new(),
        parent_class: None,
        statement_kind: None,
        function_idx: None,
    })
}
