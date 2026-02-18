#![allow(non_local_definitions)]
pub mod parser;
pub mod test_utils;
pub mod import_resolver;
pub mod graph;
pub mod symbol_table;
pub mod watcher;
pub mod incremental_parser;
pub mod cpg;
pub mod cfg;
pub mod dataflow;
pub mod callgraph;

use pyo3::prelude::*;
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyRuntimeError};
use pyo3::types::PyDict;
use std::time::Duration;
use std::path::{Path, PathBuf};
use ignore::{WalkBuilder, DirEntry};
use std::collections::HashSet;
use crate::parser::{ParserPool, SupportedLanguage, create_skeleton_from_source};
use crate::graph::GraphStatistics;
use tree_sitter::{Node, Parser as TreeSitterParser, TreeCursor};


// Create a Python exception type for our custom error.
create_exception!(semantic_engine, GraphError, PyException);
create_exception!(semantic_engine, ParseError, GraphError);
create_exception!(semantic_engine, NodeNotFoundError, GraphError);


// Implement the conversion from our internal Rust error to the Python exception.
// This allows us to use the `?` operator in our PyO3 methods for clean error handling.
impl From<graph::GraphError> for PyErr {
    fn from(err: graph::GraphError) -> PyErr {
        match err {
            graph::GraphError::ParseError(path) => {
                ParseError::new_err(format!(
                    "Syntax error in {}: unable to parse file", 
                    path.display()
                ))
            }
            graph::GraphError::NodeNotFound(path) => {
                NodeNotFoundError::new_err(format!(
                    "File not found in graph: {}", 
                    path.display()
                ))
            }
            graph::GraphError::IoError(msg) => {
                GraphError::new_err(format!("I/O error: {}", msg))
            }
            graph::GraphError::UnsupportedLanguage(path) => {
                GraphError::new_err(format!(
                    "Unsupported language for file: {}",
                    path.display()
                ))
            }
        }
    }
}

/// A simple struct to hold the result of a syntax check.
#[pyclass(name="SyntaxCheckResult")]
struct PySyntaxCheckResult {
    #[pyo3(get)]
    is_valid: bool,
    #[pyo3(get)]
    line: usize,
    #[pyo3(get)]
    message: String,
}

/// Helper function to find the first error node in a tree-sitter tree.
fn find_first_error_node<'a>(cursor: &mut TreeCursor<'a>) -> Option<Node<'a>> {
    let node = cursor.node();
    
    // Check for both ERROR nodes and MISSING nodes
    if node.is_error() || node.is_missing() {
        return Some(node);
    }
    
    // Additionally check for nodes with kind "ERROR"
    if node.kind() == "ERROR" {
        return Some(node);
    }
    
    if cursor.goto_first_child() {
        loop {
            if let Some(error_node) = find_first_error_node(cursor) {
                return Some(error_node);
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
    None
}

#[pyfunction]
fn check_syntax(content: &str, lang_name: &str) -> PyResult<PySyntaxCheckResult> {
    // For Python, use Python's native parser for definitive accuracy
    if lang_name == "python" || lang_name == "py" {
        return check_python_syntax_native(content);
    }
    
    // For other languages, use tree-sitter
    let lang = SupportedLanguage::from_extension(lang_name);
    if lang == SupportedLanguage::Unknown {
        return Ok(PySyntaxCheckResult { 
            is_valid: true, 
            line: 0, 
            message: "".to_string() 
        });
    }

    check_syntax_with_treesitter(content, lang)
}

fn check_python_syntax_native(content: &str) -> PyResult<PySyntaxCheckResult> {
    Python::with_gil(|py| {
        let compile = py.eval("compile", None, None)?;
        
        match compile.call1((content, "<string>", "exec")) {
            Ok(_) => Ok(PySyntaxCheckResult {
                is_valid: true,
                line: 0,
                message: "".to_string(),
            }),
            Err(e) => {
                // Extract line number and message from SyntaxError
                let line = if let Ok(syntax_err) = e.value(py).getattr("lineno") {
                    syntax_err.extract::<usize>().unwrap_or(0)
                } else {
                    0
                };
                
                let message = format!("Syntax error: {}", e.value(py));
                
                Ok(PySyntaxCheckResult {
                    is_valid: false,
                    line,
                    message,
                })
            }
        }
    })
}

fn check_syntax_with_treesitter(content: &str, lang: SupportedLanguage) -> PyResult<PySyntaxCheckResult> {
    let mut parser = TreeSitterParser::new();
    let ts_lang = lang.get_parser()
        .ok_or_else(|| PyRuntimeError::new_err("Could not get parser for language"))?;
    parser.set_language(ts_lang)
        .map_err(|e| PyRuntimeError::new_err(format!("Could not set language: {}", e)))?;
    
    let tree = parser.parse(content, None)
        .ok_or_else(|| PyRuntimeError::new_err("Parsing failed unexpectedly"))?;

    let mut cursor = tree.root_node().walk();
    let error_node = find_first_error_node(&mut cursor);

    if let Some(node) = error_node {
        let line = node.start_position().row + 1;
        let msg = format!(
            "Syntax error near '{}'", 
            node.utf8_text(content.as_bytes()).unwrap_or("[non-utf8 text]")
        );
        Ok(PySyntaxCheckResult {
            is_valid: false,
            line,
            message: msg,
        })
    } else {
        Ok(PySyntaxCheckResult { 
            is_valid: true, 
            line: 0, 
            message: "".to_string() 
        })
    }
}


#[pyfunction]
#[pyo3(signature = (path, ignored_dirs = None))]
fn scan_repository(path: &str, ignored_dirs: Option<Vec<String>>) -> PyResult<Vec<String>> {
    let mut files = Vec::new();
    let ignored_set: HashSet<String> = ignored_dirs.unwrap_or_default().into_iter().collect();

    let walker = WalkBuilder::new(path)
        .hidden(false)
        .git_ignore(true)
        .git_global(true) // Respect global gitignore
        .parents(true) // Respect .gitignore files in parent directories
        .filter_entry(move |entry: &DirEntry| {
            if entry.file_name() == ".git" {
                return false;
            }
            if let Some(file_type) = entry.file_type() {
                if file_type.is_dir() {
                    return entry.file_name().to_str()
                        .map(|s| !ignored_set.contains(s))
                        .unwrap_or(true);
                }
            }
            true // Keep files
        })
        .build();

    for result in walker {
        if let Ok(entry) = result {
            if let Ok(canonical_path) = entry.path().canonicalize() {
                if canonical_path.is_file() {
                    if let Some(path_str) = canonical_path.to_str() {
                        files.push(path_str.to_string());
                    }
                }
            }
        }
    }
    
    Ok(files)
}

/// Python-facing wrapper for the main repository graph.
#[pyclass(name = "RepoGraph")]
pub struct PyRepoGraph {
    graph: graph::RepoGraph,
}

/// Python-facing wrapper for the result of a graph update operation.
#[pyclass(name = "GraphUpdateResult")]
#[derive(Clone)]
pub struct PyGraphUpdateResult {
    #[pyo3(get)]
    pub edges_added: usize,
    #[pyo3(get)]
    pub edges_removed: usize,
    #[pyo3(get)]
    pub needs_pagerank_recalc: bool,
}

impl From<graph::UpdateResult> for PyGraphUpdateResult {
    fn from(res: graph::UpdateResult) -> Self {
        Self {
            edges_added: res.edges_added,
            edges_removed: res.edges_removed,
            needs_pagerank_recalc: res.needs_pagerank_recalc,
        }
    }
}

#[pyclass(name = "GraphStatistics")]
pub struct PyGraphStatistics {
    #[pyo3(get)]
    pub node_count: usize,
    #[pyo3(get)]
    pub edge_count: usize,
    #[pyo3(get)]
    pub import_edges: usize,
    #[pyo3(get)]
    pub symbol_edges: usize,
    #[pyo3(get)]
    pub total_definitions: usize,
    #[pyo3(get)]
    pub total_files_with_usages: usize,
}

impl From<GraphStatistics> for PyGraphStatistics {
    fn from(stats: GraphStatistics) -> Self {
        Self {
            node_count: stats.node_count,
            edge_count: stats.edge_count,
            import_edges: stats.import_edges,
            symbol_edges: stats.symbol_edges,
            total_definitions: stats.total_definitions,
            total_files_with_usages: stats.total_files_with_usages,
        }
    }
}


#[pymethods]
impl PyRepoGraph {
    #[new]
    #[pyo3(signature = (project_root, language = "python", ignored_dirs = None))]
    fn new(project_root: &str, language: &str, ignored_dirs: Option<Vec<String>>) -> PyResult<Self> {
        let root_path = Path::new(project_root);
        let dirs = ignored_dirs.unwrap_or_default();
        Ok(Self {
            graph: graph::RepoGraph::new(root_path, language, &dirs),
        })
    }

    /// Build the entire graph from a list of file paths.
    fn build_complete(&mut self, file_paths: Vec<String>) {
        let paths: Vec<PathBuf> = file_paths.into_iter().map(PathBuf::from).collect();
        self.graph.build_complete(&paths, &self.graph.project_root.clone());
    }

    /// Add or update a file in the graph.
    ///
    /// If the file already exists in the graph, it is first removed and then re-added
    /// with the new content (destructive upsert). This ensures a clean state.
    ///
    /// Args:
    ///     path: Project-root-relative path (must be canonical)
    ///     content: File content as string
    ///
    /// Raises:
    ///     ParseError: If file has syntax errors
    ///     GraphError: On other graph operation failures
    fn add_file(&mut self, path: String, content: String) -> PyResult<()> {
        let path_buf = PathBuf::from(path);
        self.graph.add_file(path_buf, &content)
            .map_err(|e| e.into())
    }

    /// Remove a file from the graph.
    ///
    /// This removes the file node and all associated edges. If other files were
    /// importing this file, they will be downgraded to "unresolved import" status
    /// and will automatically reconnect if this file is re-added later.
    ///
    /// Args:
    ///     path: Project-root-relative path (must match stored path exactly)
    ///
    /// Raises:
    ///     NodeNotFoundError: If file is not in the graph
    ///     GraphError: On other graph operation failures
    fn remove_file(&mut self, path: String) -> PyResult<()> {
        let path_buf = PathBuf::from(path);
        self.graph.remove_file(&path_buf)
            .map_err(|e| e.into())
    }

    /// Update a single file in the graph with its new content.
    /// 
    /// ARGS:
    ///   file_path: The absolute path to the file.
    ///   content: The new content of the file.
    /// 
    /// PERFORMANCE:
    ///   This function does NOT read from disk. It relies on the caller (Python)
    ///   to provide the content, enabling efficient in-memory updates from the watcher.
    fn update_file(&mut self, file_path: &str, content: &str) -> PyResult<PyGraphUpdateResult> {
        let path = PathBuf::from(file_path);
        // Call graph logic directly with content string. The `?` will handle the error conversion.
        let result = self.graph.update_file(&path, content)?;
        Ok(result.into())
    }

    /// Ensure PageRank scores are up-to-date before querying.
    fn ensure_pagerank_up_to_date(&mut self) {
        self.graph.ensure_pagerank_up_to_date();
    }

    /// Generate a text map of the repository's architecture.
    fn generate_map(&mut self, max_files: usize) -> String {
        self.graph.generate_map(max_files)
    }

    /// Get statistics about the graph.
    fn get_statistics(&self) -> PyGraphStatistics {
        self.graph.get_statistics().into()
    }

    /// Get incoming dependencies for a file.
    /// Returns a list of (file_path, edge_kind_str) tuples.
    fn get_dependents(&self, file_path: &str) -> PyResult<Vec<(String, String)>> {
        let path = PathBuf::from(file_path);
        let dependencies = self.graph.get_dependents(&path);
        Ok(dependencies.into_iter().map(|(p, k)| (p.to_string_lossy().into_owned(), format!("{:?}", k))).collect())
    }

    /// Get outgoing dependencies for a file.
    /// Returns a list of (file_path, edge_kind_str) tuples.
    fn get_dependencies(&self, file_path: &str) -> PyResult<Vec<(String, String)>> {
        let path = PathBuf::from(file_path);
        let dependencies = self.graph.get_dependencies(&path);
        Ok(dependencies.into_iter().map(|(p, k)| (p.to_string_lossy().into_owned(), format!("{:?}", k))).collect())
    }

    fn get_top_ranked_files(&mut self, limit: usize) -> Vec<(String, f64)> {
        self.graph.get_top_ranked_files(limit)
            .into_iter()
            .map(|(path, rank): (PathBuf, f64)| (path.to_string_lossy().into_owned(), rank))
            .collect()
    }

    /// Get skeleton for a file (Python-exposed)
    #[pyo3(name = "get_skeleton")]
    pub fn get_skeleton(&self, path: String) -> PyResult<String> {
        let skeleton_arc = self.graph.get_skeleton(Path::new(&path))
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                format!("Failed to get skeleton: {}", e)
            ))?;
        Ok(skeleton_arc.as_ref().clone())
    }

    /// Enable the CPG overlay layer for sub-file granularity.
    fn enable_cpg(&mut self) {
        self.graph.enable_cpg();
    }

    /// Enable CPG and build sub-file data for all files already in the graph.
    /// Use this when CPG is enabled after build_complete().
    fn enable_cpg_and_build(&mut self) {
        self.graph.enable_cpg_and_build();
    }

    /// Check if CPG is enabled.
    fn cpg_enabled(&self) -> bool {
        self.graph.cpg.is_some()
    }

    /// Get all functions/methods in a file as a list of dicts.
    fn get_functions_in_file(&self, py: Python, file_path: &str) -> PyResult<Vec<PyObject>> {
        let cpg = self.graph.cpg.as_ref().ok_or_else(|| {
            PyRuntimeError::new_err("CPG not enabled. Call enable_cpg() first.")
        })?;
        let path = PathBuf::from(file_path);
        let nodes = cpg.get_functions_in_file(&path);
        let mut result = Vec::new();
        for node in nodes {
            let dict = PyDict::new(py);
            dict.set_item("name", &node.name)?;
            dict.set_item("kind", match node.kind {
                cpg::CpgNodeKind::Function => "function",
                cpg::CpgNodeKind::Method => "method",
                cpg::CpgNodeKind::Class => "class",
                cpg::CpgNodeKind::Variable => "variable",
                cpg::CpgNodeKind::Statement => "statement",
                cpg::CpgNodeKind::CfgEntry => "cfg_entry",
                cpg::CpgNodeKind::CfgExit => "cfg_exit",
            })?;
            dict.set_item("start_line", node.start_line)?;
            dict.set_item("end_line", node.end_line)?;
            let params: Vec<PyObject> = node.parameters.iter().map(|p| {
                let pd = PyDict::new(py);
                pd.set_item("name", &p.name).unwrap();
                pd.set_item("type_annotation", &p.type_annotation).unwrap();
                pd.set_item("default_value", &p.default_value).unwrap();
                pd.into_py(py)
            }).collect();
            dict.set_item("parameters", params)?;
            dict.set_item("return_type", &node.return_type)?;
            dict.set_item("docstring", &node.docstring)?;
            dict.set_item("parent_class", &node.parent_class)?;
            result.push(dict.into_py(py));
        }
        Ok(result)
    }

    /// Get all CPG nodes for a file as a list of dicts (with children for classes).
    fn get_cpg_nodes(&self, py: Python, file_path: &str) -> PyResult<Vec<PyObject>> {
        let cpg = self.graph.cpg.as_ref().ok_or_else(|| {
            PyRuntimeError::new_err("CPG not enabled. Call enable_cpg() first.")
        })?;
        let path = PathBuf::from(file_path);
        let indices = match cpg.file_to_nodes.get(&path) {
            Some(indices) => indices.clone(),
            None => return Ok(Vec::new()),
        };

        let mut result = Vec::new();
        for idx in &indices {
            if let Some(node) = cpg.graph.node_weight(*idx) {
                let dict = cpg_node_to_dict(py, node)?;

                // For classes, include children
                if node.kind == cpg::CpgNodeKind::Class {
                    let children = cpg.get_children(*idx);
                    let child_dicts: Vec<PyObject> = children
                        .iter()
                        .filter_map(|(_, child_node)| {
                            cpg_node_to_dict(py, child_node).ok().map(|d| d.into_py(py))
                        })
                        .collect();
                    dict.set_item("children", child_dicts)?;
                }

                result.push(dict.into_py(py));
            }
        }
        Ok(result)
    }

    /// Get the CFG for a function as a list of (source_line, target_line, edge_kind) tuples.
    fn get_function_cfg(&self, file_path: &str, function_name: &str) -> PyResult<Vec<(usize, usize, String)>> {
        let cpg = self.graph.cpg.as_ref().ok_or_else(|| {
            PyRuntimeError::new_err("CPG not enabled. Call enable_cpg() first.")
        })?;
        let path = PathBuf::from(file_path);
        let indices = match cpg.file_to_nodes.get(&path) {
            Some(indices) => indices.clone(),
            None => return Ok(Vec::new()),
        };

        // Find the function node
        let func_idx = indices.iter().find(|idx| {
            cpg.graph.node_weight(**idx)
                .map(|n| {
                    matches!(n.kind, cpg::CpgNodeKind::Function | cpg::CpgNodeKind::Method)
                        && n.name == function_name
                })
                .unwrap_or(false)
        });

        let func_idx = match func_idx {
            Some(idx) => *idx,
            None => return Ok(Vec::new()),
        };

        let edges = cpg.get_cfg_edges_for_function(func_idx);
        let result: Vec<(usize, usize, String)> = edges.iter().map(|(src, tgt, edge)| {
            let src_line = cpg.graph.node_weight(*src).map(|n| n.start_line).unwrap_or(0);
            let tgt_line = cpg.graph.node_weight(*tgt).map(|n| n.start_line).unwrap_or(0);
            let kind = format!("{:?}", edge);
            (src_line, tgt_line, kind)
        }).collect();
        Ok(result)
    }

    /// Get data flow edges for a function as (def_line, use_line, var_name) tuples.
    fn get_function_dataflow(&self, file_path: &str, function_name: &str) -> PyResult<Vec<(usize, usize, String)>> {
        let cpg = self.graph.cpg.as_ref().ok_or_else(|| {
            PyRuntimeError::new_err("CPG not enabled. Call enable_cpg() first.")
        })?;
        let path = PathBuf::from(file_path);
        let indices = match cpg.file_to_nodes.get(&path) {
            Some(indices) => indices.clone(),
            None => return Ok(Vec::new()),
        };

        // Find the function node
        let func_idx = indices.iter().find(|idx| {
            cpg.graph.node_weight(**idx)
                .map(|n| {
                    matches!(n.kind, cpg::CpgNodeKind::Function | cpg::CpgNodeKind::Method)
                        && n.name == function_name
                })
                .unwrap_or(false)
        });

        let func_idx = match func_idx {
            Some(idx) => *idx,
            None => return Ok(Vec::new()),
        };

        let edges = cpg.get_dataflow_edges_for_function(func_idx);
        let result: Vec<(usize, usize, String)> = edges.iter().map(|(src, tgt, var)| {
            let src_line = cpg.graph.node_weight(*src).map(|n| n.start_line).unwrap_or(0);
            let tgt_line = cpg.graph.node_weight(*tgt).map(|n| n.start_line).unwrap_or(0);
            (src_line, tgt_line, var.to_string())
        }).collect();
        Ok(result)
    }

    /// Get defs/uses for each statement in a function.
    fn get_statement_defs_uses(&self, py: Python, file_path: &str, function_name: &str) -> PyResult<Vec<PyObject>> {
        let cpg = self.graph.cpg.as_ref().ok_or_else(|| {
            PyRuntimeError::new_err("CPG not enabled. Call enable_cpg() first.")
        })?;
        let path = PathBuf::from(file_path);
        let indices = match cpg.file_to_nodes.get(&path) {
            Some(indices) => indices.clone(),
            None => return Ok(Vec::new()),
        };

        // Find the function node
        let func_idx = indices.iter().find(|idx| {
            cpg.graph.node_weight(**idx)
                .map(|n| {
                    matches!(n.kind, cpg::CpgNodeKind::Function | cpg::CpgNodeKind::Method)
                        && n.name == function_name
                })
                .unwrap_or(false)
        });

        let func_idx = match func_idx {
            Some(idx) => *idx,
            None => return Ok(Vec::new()),
        };

        let mut result = Vec::new();
        for idx in cpg.graph.node_indices() {
            if let Some(node) = cpg.graph.node_weight(idx) {
                if node.function_idx == Some(func_idx)
                    && matches!(node.kind, cpg::CpgNodeKind::Statement | cpg::CpgNodeKind::CfgEntry | cpg::CpgNodeKind::CfgExit)
                {
                    let dict = PyDict::new(py);
                    dict.set_item("name", &node.name)?;
                    dict.set_item("start_line", node.start_line)?;
                    let defs: Vec<String> = cpg.stmt_defs.get(&idx).cloned().unwrap_or_default();
                    let uses: Vec<String> = cpg.stmt_uses.get(&idx).cloned().unwrap_or_default();
                    dict.set_item("defs", defs)?;
                    dict.set_item("uses", uses)?;
                    result.push(dict.into_py(py));
                }
            }
        }
        Ok(result)
    }

    /// Get CFG statement nodes for a function as a list of dicts.
    fn get_cfg_statements(&self, py: Python, file_path: &str, function_name: &str) -> PyResult<Vec<PyObject>> {
        let cpg = self.graph.cpg.as_ref().ok_or_else(|| {
            PyRuntimeError::new_err("CPG not enabled. Call enable_cpg() first.")
        })?;
        let path = PathBuf::from(file_path);
        let indices = match cpg.file_to_nodes.get(&path) {
            Some(indices) => indices.clone(),
            None => return Ok(Vec::new()),
        };

        // Find the function node
        let func_idx = indices.iter().find(|idx| {
            cpg.graph.node_weight(**idx)
                .map(|n| {
                    matches!(n.kind, cpg::CpgNodeKind::Function | cpg::CpgNodeKind::Method)
                        && n.name == function_name
                })
                .unwrap_or(false)
        });

        let func_idx = match func_idx {
            Some(idx) => *idx,
            None => return Ok(Vec::new()),
        };

        let mut result = Vec::new();
        for idx in cpg.graph.node_indices() {
            if let Some(node) = cpg.graph.node_weight(idx) {
                if node.function_idx == Some(func_idx)
                    && matches!(node.kind, cpg::CpgNodeKind::Statement | cpg::CpgNodeKind::CfgEntry | cpg::CpgNodeKind::CfgExit)
                {
                    let dict = PyDict::new(py);
                    dict.set_item("name", &node.name)?;
                    dict.set_item("kind", match node.kind {
                        cpg::CpgNodeKind::Statement => "statement",
                        cpg::CpgNodeKind::CfgEntry => "cfg_entry",
                        cpg::CpgNodeKind::CfgExit => "cfg_exit",
                        _ => "unknown",
                    })?;
                    dict.set_item("start_line", node.start_line)?;
                    dict.set_item("end_line", node.end_line)?;
                    if let Some(ref sk) = node.statement_kind {
                        dict.set_item("statement_kind", format!("{:?}", sk))?;
                    }
                    result.push(dict.into_py(py));
                }
            }
        }
        Ok(result)
    }

    /// Get all functions called by a given function.
    /// Returns a list of dicts: {name, file, line}
    fn get_callees(&self, py: Python, file_path: &str, function_name: &str) -> PyResult<Vec<PyObject>> {
        let cpg = self.graph.cpg.as_ref().ok_or_else(|| {
            PyRuntimeError::new_err("CPG not enabled. Call enable_cpg() first.")
        })?;
        let path = PathBuf::from(file_path);
        let func_idx = find_func_idx(cpg, &path, function_name);
        let func_idx = match func_idx {
            Some(idx) => idx,
            None => return Ok(Vec::new()),
        };

        let callees = cpg.get_callees(func_idx);
        let mut result = Vec::new();
        for (_idx, node) in callees {
            let dict = PyDict::new(py);
            dict.set_item("name", &node.name)?;
            dict.set_item("file", node.file_path.to_string_lossy().as_ref())?;
            dict.set_item("line", node.start_line)?;
            result.push(dict.into_py(py));
        }
        Ok(result)
    }

    /// Get all functions that call a given function.
    /// Returns a list of dicts: {name, file, line}
    fn get_callers(&self, py: Python, file_path: &str, function_name: &str) -> PyResult<Vec<PyObject>> {
        let cpg = self.graph.cpg.as_ref().ok_or_else(|| {
            PyRuntimeError::new_err("CPG not enabled. Call enable_cpg() first.")
        })?;
        let path = PathBuf::from(file_path);
        let func_idx = find_func_idx(cpg, &path, function_name);
        let func_idx = match func_idx {
            Some(idx) => idx,
            None => return Ok(Vec::new()),
        };

        let callers = cpg.get_callers(func_idx);
        let mut result = Vec::new();
        for (_idx, node) in callers {
            let dict = PyDict::new(py);
            dict.set_item("name", &node.name)?;
            dict.set_item("file", node.file_path.to_string_lossy().as_ref())?;
            dict.set_item("line", node.start_line)?;
            result.push(dict.into_py(py));
        }
        Ok(result)
    }

    /// Explicitly trigger call graph resolution (Pass 2).
    fn resolve_call_graph(&mut self) -> PyResult<()> {
        let cpg = self.graph.cpg.as_mut().ok_or_else(|| {
            PyRuntimeError::new_err("CPG not enabled. Call enable_cpg() first.")
        })?;
        crate::callgraph::CallGraphBuilder::resolve_all(cpg, &self.graph.symbol_index);
        Ok(())
    }
}

/// Helper to find a function/method NodeIndex by file path and name.
fn find_func_idx(cpg: &cpg::CpgLayer, path: &PathBuf, function_name: &str) -> Option<petgraph::graph::NodeIndex> {
    cpg.file_to_nodes.get(path)?.iter().find(|idx| {
        cpg.graph.node_weight(**idx)
            .map(|n| {
                matches!(n.kind, cpg::CpgNodeKind::Function | cpg::CpgNodeKind::Method)
                    && n.name == function_name
            })
            .unwrap_or(false)
    }).copied()
}

/// Helper to convert a CpgNode to a Python dict.
fn cpg_node_to_dict<'py>(py: Python<'py>, node: &cpg::CpgNode) -> PyResult<&'py PyDict> {
    let dict = PyDict::new(py);
    dict.set_item("name", &node.name)?;
    dict.set_item("kind", match node.kind {
        cpg::CpgNodeKind::Function => "function",
        cpg::CpgNodeKind::Method => "method",
        cpg::CpgNodeKind::Class => "class",
        cpg::CpgNodeKind::Variable => "variable",
        cpg::CpgNodeKind::Statement => "statement",
        cpg::CpgNodeKind::CfgEntry => "cfg_entry",
        cpg::CpgNodeKind::CfgExit => "cfg_exit",
    })?;
    dict.set_item("start_line", node.start_line)?;
    dict.set_item("end_line", node.end_line)?;
    dict.set_item("start_byte", node.start_byte)?;
    dict.set_item("end_byte", node.end_byte)?;
    let params: Vec<PyObject> = node.parameters.iter().map(|p| {
        let pd = PyDict::new(py);
        pd.set_item("name", &p.name).unwrap();
        pd.set_item("type_annotation", &p.type_annotation).unwrap();
        pd.set_item("default_value", &p.default_value).unwrap();
        pd.into_py(py)
    }).collect();
    dict.set_item("parameters", params)?;
    dict.set_item("return_type", &node.return_type)?;
    dict.set_item("docstring", &node.docstring)?;
    dict.set_item("bases", &node.bases)?;
    dict.set_item("parent_class", &node.parent_class)?;
    Ok(dict)
}


/// Python-facing file change event
#[pyclass(name = "FileChangeEvent")]
#[derive(Clone)]
pub struct PyFileChangeEvent {
    #[pyo3(get)]
    pub event_type: String,
    
    #[pyo3(get)]
    pub path: String,
}

#[pymethods]
impl PyFileChangeEvent {
    fn __repr__(&self) -> String {
        format!("FileChangeEvent(type='{}', path='{}')", self.event_type, self.path)
    }
    
    fn __str__(&self) -> String {
        format!("{}: {}", self.event_type, self.path)
    }
}

impl From<watcher::FileChangeEvent> for PyFileChangeEvent {
    fn from(event: watcher::FileChangeEvent) -> Self {
        match event {
            watcher::FileChangeEvent::Created(path) => PyFileChangeEvent {
                event_type: "created".to_string(),
                path: path.display().to_string(),
            },
            watcher::FileChangeEvent::Modified(path) => PyFileChangeEvent {
                event_type: "modified".to_string(),
                path: path.display().to_string(),
            },
            watcher::FileChangeEvent::Deleted(path) => PyFileChangeEvent {
                event_type: "deleted".to_string(),
                path: path.display().to_string(),
            },
            watcher::FileChangeEvent::Renamed { from, to } => PyFileChangeEvent {
                event_type: "renamed".to_string(),
                path: format!("{} -> {}", from.display(), to.display()),
            },
        }
    }
}

/// Python-facing watcher statistics
#[pyclass(name = "WatcherStats")]
#[derive(Clone)]
pub struct PyWatcherStats {
    #[pyo3(get)]
    pub events_received: usize,
    
    #[pyo3(get)]
    pub events_filtered: usize,
    
    #[pyo3(get)]
    pub errors_encountered: usize,
}

#[pymethods]
impl PyWatcherStats {
    fn __repr__(&self) -> String {
        format!(
            "WatcherStats(received={}, filtered={}, errors={})",
            self.events_received, self.events_filtered, self.errors_encountered
        )
    }
}

impl From<watcher::WatcherStats> for PyWatcherStats {
    fn from(stats: watcher::WatcherStats) -> Self {
        Self {
            events_received: stats.events_received,
            events_filtered: stats.events_filtered,
            errors_encountered: stats.errors_encountered,
        }
    }
}

/// Python-facing file watcher
#[pyclass(name = "FileWatcher")]
pub struct PyFileWatcher {
    watcher: Option<watcher::FileWatcher>,
}

#[pymethods]
impl PyFileWatcher {
    #[new]
    #[pyo3(signature = (path, extensions=None, ignored_dirs=None))]
    fn new(
        path: String,
        extensions: Option<Vec<String>>,
        ignored_dirs: Option<Vec<String>>,
    ) -> PyResult<Self> {
        let mut filter = watcher::FileFilter::default();
        
        if let Some(exts) = extensions {
            filter.extensions = exts;
        }
        
        if let Some(dirs) = ignored_dirs {
            filter.ignored_dirs.extend(dirs);
        }
        
        let watcher = watcher::FileWatcher::new(PathBuf::from(path), filter)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to start watcher: {}", e)))?;
        
        Ok(Self {
            watcher: Some(watcher),
        })
    }
    
    /// Poll for new events (non-blocking)
    fn poll_events(&self) -> PyResult<Vec<PyFileChangeEvent>> {
        let watcher = self.watcher.as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Watcher has been stopped"))?;
        
        Ok(watcher.poll_events()
            .into_iter()
            .map(PyFileChangeEvent::from)
            .collect())
    }
    
    /// Wait for next event with timeout (blocking)
    fn wait_for_event(&self, timeout_ms: u64) -> PyResult<Option<PyFileChangeEvent>> {
        let watcher = self.watcher.as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Watcher has been stopped"))?;
        
        // FIXED: Removed .map_err() and '?'
        // wait_for_event returns Option<FileChangeEvent>, so we just map the inner value if it exists.
        Ok(watcher.wait_for_event(Duration::from_millis(timeout_ms))
            .map(PyFileChangeEvent::from))
    }
    
    /// Get statistics
    fn get_stats(&self) -> PyResult<PyWatcherStats> {
        let watcher = self.watcher.as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Watcher has been stopped"))?;
            
        Ok(PyWatcherStats::from(watcher.get_stats()))
    }
    
        /// Stop the watcher
    
        fn stop(&mut self) {
    
            // dropping the watcher stops it
    
            self.watcher = None;
    
        }
    
    }
    
    
    
    /// Atlas Semantic Engine
/// 
/// A multi-language code analysis engine supporting Python, JavaScript, and TypeScript.
#[pymodule]
fn semantic_engine(_py: Python, m: &PyModule) -> PyResult<()> {

    m.add_function(wrap_pyfunction!(scan_repository, m)?)?;
    m.add_function(wrap_pyfunction!(check_syntax, m)?)?;
    m.add_function(wrap_pyfunction!(create_skeleton_from_source, m)?)?;

    m.add_class::<PyRepoGraph>()?;
    m.add_class::<PyGraphStatistics>()?;

    m.add_class::<PyFileWatcher>()?;

    m.add_class::<PyGraphUpdateResult>()?;

    m.add_class::<PyFileChangeEvent>()?;

    m.add_class::<PyWatcherStats>()?;
    m.add_class::<PySyntaxCheckResult>()?;

    m.add("GraphError", _py.get_type::<GraphError>())?;
    m.add("ParseError", _py.get_type::<ParseError>())?;
    m.add("NodeNotFoundError", _py.get_type::<NodeNotFoundError>())?;

    Ok(())

}
