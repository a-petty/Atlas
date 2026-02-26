use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use crate::cpg::{CpgEdge, CpgLayer, CpgNodeKind, StatementKind};
use crate::symbol_table::SymbolIndex;

/// A call site extracted from a statement within a function.
#[derive(Debug, Clone)]
pub struct CallSite {
    /// The Statement node containing the call expression.
    pub stmt_idx: NodeIndex,
    /// The owning Function/Method node.
    pub caller_func_idx: NodeIndex,
    /// The callee name: "foo" for simple calls, "method" for self.method().
    pub callee_name: String,
    /// The receiver object: "self", "obj", "module", etc.
    pub receiver: Option<String>,
    /// Positional argument expression texts.
    pub positional_args: Vec<String>,
    /// Keyword arguments as (name, value_text) pairs.
    pub keyword_args: Vec<(String, String)>,
    /// Start byte of the call expression in source.
    pub call_start_byte: usize,
    /// End byte of the call expression in source.
    pub call_end_byte: usize,
}

/// Result of attempting to resolve a call site to a callee function.
#[derive(Debug)]
pub enum CallResolution {
    Resolved(NodeIndex),
    Unresolved(String),
}

/// Common Python builtins that should not be resolved.
fn python_builtins() -> HashSet<&'static str> {
    [
        "print", "len", "range", "int", "str", "list", "dict", "set", "tuple",
        "type", "isinstance", "issubclass", "open", "super", "enumerate", "zip",
        "map", "filter", "sorted", "min", "max", "sum", "abs", "hasattr",
        "getattr", "setattr", "delattr", "callable", "repr", "hash", "id",
        "input", "iter", "next", "reversed", "round", "bool", "bytes",
        "bytearray", "memoryview", "float", "complex", "frozenset", "object",
        "property", "staticmethod", "classmethod", "vars", "dir", "hex", "oct",
        "bin", "ord", "chr", "format", "any", "all", "breakpoint", "compile",
        "eval", "exec", "globals", "locals", "pow", "divmod", "slice",
        "ValueError", "TypeError", "KeyError", "IndexError", "AttributeError",
        "RuntimeError", "Exception", "StopIteration", "NotImplementedError",
        "OSError", "IOError", "FileNotFoundError", "ImportError",
    ].into_iter().collect()
}

/// Stateless builder for call graph construction.
pub struct CallGraphBuilder;

impl CallGraphBuilder {
    // -----------------------------------------------------------------------
    // Pass 1: Extract call sites from a single function
    // -----------------------------------------------------------------------

    /// Extract all call sites from statements within a function.
    /// Called during `build_file` after CFG and dataflow analysis.
    pub fn extract_call_sites(
        cpg: &CpgLayer,
        func_idx: NodeIndex,
        source: &str,
    ) -> Vec<CallSite> {
        let file_path = match cpg.graph.node_weight(func_idx) {
            Some(n) => n.file_path.clone(),
            None => return Vec::new(),
        };

        let tree = match cpg.trees.get(&file_path) {
            Some(t) => t.clone(),
            None => return Vec::new(),
        };

        let mut sites = Vec::new();

        // Iterate over Statement nodes owned by this function (via index)
        for &idx in cpg.get_stmts_for_function(func_idx) {
            if let Some(node) = cpg.graph.node_weight(idx) {
                if node.kind == CpgNodeKind::Statement {
                    if let Some(ts_node) = find_node_at_bytes(
                        tree.root_node(),
                        node.start_byte,
                        node.end_byte,
                    ) {
                        extract_calls_from_node(ts_node, source, idx, func_idx, &mut sites);
                    }
                }
            }
        }

        sites
    }

    // -----------------------------------------------------------------------
    // Pass 2: Resolve all call sites across the entire CPG
    // -----------------------------------------------------------------------

    /// Resolve all call sites after all files have been built.
    /// Creates Calls/CalledBy edges and DataFlowArgument/DataFlowReturn edges.
    pub fn resolve_all(cpg: &mut CpgLayer, symbol_index: &SymbolIndex) {
        let resolve_start = Instant::now();
        let builtins = python_builtins();

        // Collect all call sites with their caller file paths
        let collect_start = Instant::now();
        let all_sites: Vec<(CallSite, std::path::PathBuf)> = cpg
            .call_sites
            .values()
            .flat_map(|sites| sites.iter())
            .filter_map(|site| {
                cpg.graph
                    .node_weight(site.caller_func_idx)
                    .map(|n| (site.clone(), n.file_path.clone()))
            })
            .collect();
        eprintln!("[DIAG] resolve_all: collected {} call sites from {} functions in {:.2}s",
            all_sites.len(), cpg.call_sites.len(), collect_start.elapsed().as_secs_f64());

        // Resolve each call site
        let mut edges_to_add: Vec<EdgeToAdd> = Vec::new();
        let mut resolved_count = 0usize;

        let resolve_loop_start = Instant::now();
        for (i, (site, caller_file)) in all_sites.iter().enumerate() {
            let site_start = Instant::now();
            if let CallResolution::Resolved(callee_idx) =
                Self::resolve_callee(cpg, site, caller_file, symbol_index, &builtins)
            {
                collect_edges_for_resolved_call(cpg, site, callee_idx, &mut edges_to_add);
                resolved_count += 1;
            }
            let site_elapsed = site_start.elapsed();
            if site_elapsed.as_millis() > 100 {
                eprintln!("[DIAG] resolve_all: slow call site #{} ({:.2}s): {} -> {} in {}",
                    i, site_elapsed.as_secs_f64(),
                    site.callee_name,
                    cpg.graph.node_weight(site.caller_func_idx)
                        .map(|n| n.name.as_str()).unwrap_or("?"),
                    caller_file.display());
            }
            if (i + 1) % 1000 == 0 {
                eprintln!("[DIAG] resolve_all: progress {}/{} sites ({} resolved, {:.2}s elapsed)",
                    i + 1, all_sites.len(), resolved_count, resolve_loop_start.elapsed().as_secs_f64());
            }
        }
        eprintln!("[DIAG] resolve_all: resolution loop completed in {:.2}s ({}/{} resolved)",
            resolve_loop_start.elapsed().as_secs_f64(), resolved_count, all_sites.len());

        // Apply all edges
        let apply_start = Instant::now();
        for edge in edges_to_add {
            cpg.graph.add_edge(edge.source, edge.target, edge.weight);
        }
        eprintln!("[DIAG] resolve_all: edge application completed in {:.2}s", apply_start.elapsed().as_secs_f64());
        eprintln!("[DIAG] resolve_all: total time {:.2}s", resolve_start.elapsed().as_secs_f64());
    }

    // -----------------------------------------------------------------------
    // Incremental: Resolve call sites for a single file
    // -----------------------------------------------------------------------

    /// After updating a file, re-resolve its call sites and any cross-file references.
    pub fn resolve_file(cpg: &mut CpgLayer, file_path: &Path, symbol_index: &SymbolIndex) {
        let builtins = python_builtins();

        // Remove existing inter-procedural edges for functions in this file
        Self::remove_interprocedural_edges_for_file(cpg, file_path);

        // Collect function names defined in this file (for cross-file re-resolution)
        let file_func_names: HashSet<String> = cpg
            .file_to_nodes
            .get(file_path)
            .map(|indices| {
                indices
                    .iter()
                    .filter_map(|idx| {
                        cpg.graph.node_weight(*idx).and_then(|n| {
                            if matches!(n.kind, CpgNodeKind::Function | CpgNodeKind::Method) {
                                Some(n.name.clone())
                            } else {
                                None
                            }
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Collect call sites from this file
        let local_sites: Vec<(CallSite, std::path::PathBuf)> = cpg
            .file_to_nodes
            .get(file_path)
            .map(|indices| {
                indices
                    .iter()
                    .filter_map(|idx| cpg.call_sites.get(idx))
                    .flat_map(|sites| sites.iter())
                    .map(|site| (site.clone(), file_path.to_path_buf()))
                    .collect()
            })
            .unwrap_or_default();

        // Also collect call sites from OTHER files that reference any of our function names
        let mut cross_file_sites: Vec<(CallSite, std::path::PathBuf)> = Vec::new();
        for (func_idx, sites) in &cpg.call_sites {
            if let Some(node) = cpg.graph.node_weight(*func_idx) {
                if node.file_path != file_path {
                    for site in sites {
                        if file_func_names.contains(&site.callee_name) {
                            // Also remove inter-proc edges from THIS caller to allow re-resolution
                            cross_file_sites.push((site.clone(), node.file_path.clone()));
                        }
                    }
                }
            }
        }

        // Remove inter-proc edges from cross-file callers that reference our names
        let cross_file_func_indices: HashSet<NodeIndex> = cross_file_sites
            .iter()
            .map(|(site, _)| site.caller_func_idx)
            .collect();
        for func_idx in &cross_file_func_indices {
            Self::remove_interprocedural_edges_for_func(cpg, *func_idx);
        }

        // Resolve all collected sites
        let mut edges_to_add: Vec<EdgeToAdd> = Vec::new();

        for (site, caller_file) in local_sites.iter().chain(cross_file_sites.iter()) {
            if let CallResolution::Resolved(callee_idx) =
                Self::resolve_callee(cpg, site, caller_file, symbol_index, &builtins)
            {
                collect_edges_for_resolved_call(cpg, site, callee_idx, &mut edges_to_add);
            }
        }

        for edge in edges_to_add {
            let already_exists = cpg.graph
                .edges_connecting(edge.source, edge.target)
                .any(|e| *e.weight() == edge.weight);
            if !already_exists {
                cpg.graph.add_edge(edge.source, edge.target, edge.weight);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Resolution logic
    // -----------------------------------------------------------------------

    /// Attempt to resolve a single call site to a callee function node.
    fn resolve_callee(
        cpg: &CpgLayer,
        site: &CallSite,
        caller_file: &Path,
        symbol_index: &SymbolIndex,
        builtins: &HashSet<&str>,
    ) -> CallResolution {
        let name = &site.callee_name;

        // 1. Filter builtins
        if builtins.contains(name.as_str()) {
            return CallResolution::Unresolved("builtin".to_string());
        }

        // 2. self.method() calls — look for sibling methods in same class
        if site.receiver.as_deref() == Some("self") || site.receiver.as_deref() == Some("cls") {
            return Self::resolve_self_method(cpg, site);
        }

        // 3. Simple name, same file first
        if site.receiver.is_none() {
            if let Some(indices) = cpg.file_to_nodes.get(caller_file) {
                let matches: Vec<NodeIndex> = indices
                    .iter()
                    .filter(|idx| {
                        cpg.graph
                            .node_weight(**idx)
                            .map(|n| {
                                matches!(n.kind, CpgNodeKind::Function | CpgNodeKind::Method)
                                    && n.name == *name
                            })
                            .unwrap_or(false)
                    })
                    .copied()
                    .collect();

                if matches.len() == 1 {
                    return CallResolution::Resolved(matches[0]);
                }
            }

            // 4. Simple name, cross-file via symbol_index
            if let Some(def_paths) = symbol_index.definitions.get(name) {
                let mut all_matches: Vec<NodeIndex> = Vec::new();
                for def_path in def_paths {
                    if let Some(indices) = cpg.file_to_nodes.get(def_path.as_path()) {
                        for &idx in indices {
                            if let Some(node) = cpg.graph.node_weight(idx) {
                                if matches!(node.kind, CpgNodeKind::Function | CpgNodeKind::Method)
                                    && node.name == *name
                                {
                                    all_matches.push(idx);
                                }
                            }
                        }
                    }
                }
                if all_matches.len() == 1 {
                    return CallResolution::Resolved(all_matches[0]);
                }
                if all_matches.len() > 1 {
                    return CallResolution::Unresolved("ambiguous".to_string());
                }
            }

            // Also check name_to_funcs for functions not in symbol_index
            if let Some(func_indices) = cpg.name_to_funcs.get(name) {
                if func_indices.len() == 1 {
                    return CallResolution::Resolved(func_indices[0]);
                }
                if func_indices.len() > 1 {
                    return CallResolution::Unresolved("ambiguous".to_string());
                }
            }
        }

        // 5. Module-qualified calls: receiver matches an imported module name
        // 6. Class-qualified calls: receiver matches an imported class name
        if let Some(receiver) = &site.receiver {
            if let Some(bindings) = cpg.import_bindings.get(caller_file) {
                for binding in bindings {
                    if binding.local_name != *receiver {
                        continue;
                    }

                    if binding.imported_symbol.is_none() {
                        // Module import: `import utils` → utils.process()
                        if let Some(indices) = cpg.file_to_nodes.get(&binding.resolved_path) {
                            let matches: Vec<NodeIndex> = indices
                                .iter()
                                .filter(|idx| {
                                    cpg.graph
                                        .node_weight(**idx)
                                        .map(|n| {
                                            matches!(
                                                n.kind,
                                                CpgNodeKind::Function | CpgNodeKind::Method
                                            ) && n.name == *name
                                        })
                                        .unwrap_or(false)
                                })
                                .copied()
                                .collect();
                            if matches.len() == 1 {
                                return CallResolution::Resolved(matches[0]);
                            }
                        }
                    } else if let Some(ref sym) = binding.imported_symbol {
                        // Class import: `from http_client import HttpClient` → HttpClient.get()
                        if let Some(indices) = cpg.file_to_nodes.get(&binding.resolved_path) {
                            let matches: Vec<NodeIndex> = indices
                                .iter()
                                .filter(|idx| {
                                    cpg.graph
                                        .node_weight(**idx)
                                        .map(|n| {
                                            n.kind == CpgNodeKind::Method
                                                && n.name == *name
                                                && n.parent_class.as_deref() == Some(sym.as_str())
                                        })
                                        .unwrap_or(false)
                                })
                                .copied()
                                .collect();
                            if matches.len() == 1 {
                                return CallResolution::Resolved(matches[0]);
                            }
                        }
                    }
                }
            }

            // Fallback: try SymbolIndex for class-qualified calls
            // (receiver is a class name defined in another file)
            if let Some(def_paths) = symbol_index.definitions.get(receiver) {
                for def_path in def_paths {
                    if let Some(indices) = cpg.file_to_nodes.get(def_path.as_path()) {
                        let matches: Vec<NodeIndex> = indices
                            .iter()
                            .filter(|idx| {
                                cpg.graph
                                    .node_weight(**idx)
                                    .map(|n| {
                                        n.kind == CpgNodeKind::Method
                                            && n.name == *name
                                            && n.parent_class.as_deref()
                                                == Some(receiver.as_str())
                                    })
                                    .unwrap_or(false)
                            })
                            .copied()
                            .collect();
                        if matches.len() == 1 {
                            return CallResolution::Resolved(matches[0]);
                        }
                    }
                }
            }
        }

        // 7. Otherwise unresolved
        CallResolution::Unresolved("unresolved".to_string())
    }

    /// Resolve a self.method() call by looking for sibling methods in the same class.
    fn resolve_self_method(cpg: &CpgLayer, site: &CallSite) -> CallResolution {
        // Find the caller's parent_class
        let caller_class = match cpg.graph.node_weight(site.caller_func_idx) {
            Some(n) => match &n.parent_class {
                Some(cls) => cls.clone(),
                None => return CallResolution::Unresolved("no_parent_class".to_string()),
            },
            None => return CallResolution::Unresolved("no_caller".to_string()),
        };

        let caller_file = cpg.graph[site.caller_func_idx].file_path.clone();

        // Find the class node and its method children
        if let Some(indices) = cpg.file_to_nodes.get(&caller_file) {
            let matches: Vec<NodeIndex> = indices
                .iter()
                .filter(|idx| {
                    cpg.graph
                        .node_weight(**idx)
                        .map(|n| {
                            matches!(n.kind, CpgNodeKind::Function | CpgNodeKind::Method)
                                && n.name == site.callee_name
                                && n.parent_class.as_deref() == Some(&caller_class)
                        })
                        .unwrap_or(false)
                })
                .copied()
                .collect();

            if matches.len() == 1 {
                return CallResolution::Resolved(matches[0]);
            }
        }

        CallResolution::Unresolved("method_not_found".to_string())
    }

    // -----------------------------------------------------------------------
    // Edge removal helpers
    // -----------------------------------------------------------------------

    /// Remove all inter-procedural edges (Calls, CalledBy, DataFlowArgument, DataFlowReturn)
    /// involving functions from a given file.
    fn remove_interprocedural_edges_for_file(cpg: &mut CpgLayer, file_path: &Path) {
        let func_indices: Vec<NodeIndex> = cpg
            .file_to_nodes
            .get(file_path)
            .map(|indices| {
                indices
                    .iter()
                    .filter(|idx| {
                        cpg.graph
                            .node_weight(**idx)
                            .map(|n| matches!(n.kind, CpgNodeKind::Function | CpgNodeKind::Method))
                            .unwrap_or(false)
                    })
                    .copied()
                    .collect()
            })
            .unwrap_or_default();

        for func_idx in func_indices {
            Self::remove_interprocedural_edges_for_func(cpg, func_idx);
        }
    }

    /// Remove all inter-procedural edges involving a specific function.
    fn remove_interprocedural_edges_for_func(cpg: &mut CpgLayer, func_idx: NodeIndex) {
        // Collect all nodes belonging to this function (for DataFlowArgument/Return)
        let mut func_nodes: Vec<NodeIndex> = vec![func_idx];
        func_nodes.extend_from_slice(cpg.get_stmts_for_function(func_idx));

        // Collect edge IDs to remove
        let mut edges_to_remove = Vec::new();
        for &node_idx in &func_nodes {
            for edge in cpg.graph.edges_directed(node_idx, petgraph::Direction::Outgoing) {
                if is_interprocedural_edge(edge.weight()) {
                    edges_to_remove.push(edge.id());
                }
            }
            for edge in cpg.graph.edges_directed(node_idx, petgraph::Direction::Incoming) {
                if is_interprocedural_edge(edge.weight()) {
                    edges_to_remove.push(edge.id());
                }
            }
        }

        // Deduplicate and remove
        edges_to_remove.sort();
        edges_to_remove.dedup();
        // Remove in reverse order to avoid index invalidation
        edges_to_remove.sort_by(|a, b| b.index().cmp(&a.index()));
        for edge_id in edges_to_remove {
            cpg.graph.remove_edge(edge_id);
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn is_interprocedural_edge(edge: &CpgEdge) -> bool {
    matches!(
        edge,
        CpgEdge::Calls
            | CpgEdge::CalledBy
            | CpgEdge::DataFlowArgument { .. }
            | CpgEdge::DataFlowReturn
    )
}

/// An edge to be added to the graph (collected before batch insertion).
struct EdgeToAdd {
    source: NodeIndex,
    target: NodeIndex,
    weight: CpgEdge,
}

/// After resolving a call, collect all edges to create.
fn collect_edges_for_resolved_call(
    cpg: &CpgLayer,
    site: &CallSite,
    callee_idx: NodeIndex,
    edges: &mut Vec<EdgeToAdd>,
) {
    // Calls: caller_func → callee_func
    edges.push(EdgeToAdd {
        source: site.caller_func_idx,
        target: callee_idx,
        weight: CpgEdge::Calls,
    });

    // CalledBy: callee_func → caller_func
    edges.push(EdgeToAdd {
        source: callee_idx,
        target: site.caller_func_idx,
        weight: CpgEdge::CalledBy,
    });

    // DataFlowArgument: call_stmt → callee's CfgEntry for each matched arg→param
    if let Some(&entry_idx) = cpg.function_to_entry.get(&callee_idx) {
        let callee_params = match cpg.graph.node_weight(callee_idx) {
            Some(n) => &n.parameters,
            None => return,
        };

        // Build list of non-self/cls parameter names with their positions
        let real_params: Vec<(usize, &str)> = callee_params
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.name != "self"
                    && p.name != "cls"
                    && !p.name.starts_with('*')
            })
            .map(|(i, p)| (i, p.name.as_str()))
            .collect();

        // Map positional args
        for (arg_pos, _arg_text) in site.positional_args.iter().enumerate() {
            if arg_pos < real_params.len() {
                edges.push(EdgeToAdd {
                    source: site.stmt_idx,
                    target: entry_idx,
                    weight: CpgEdge::DataFlowArgument { position: arg_pos },
                });
            }
        }

        // Map keyword args
        for (kw_name, _kw_value) in &site.keyword_args {
            if let Some(&(_, _)) = real_params.iter().find(|(_, pname)| pname == kw_name) {
                // Find the position in real_params
                if let Some(pos) = real_params.iter().position(|(_, pname)| pname == kw_name) {
                    edges.push(EdgeToAdd {
                        source: site.stmt_idx,
                        target: entry_idx,
                        weight: CpgEdge::DataFlowArgument { position: pos },
                    });
                }
            }
        }
    }

    // DataFlowReturn: each return statement in callee → call_stmt in caller
    for &idx in cpg.get_stmts_for_function(callee_idx) {
        if let Some(node) = cpg.graph.node_weight(idx) {
            if node.kind == CpgNodeKind::Statement
                && node.statement_kind.as_ref() == Some(&StatementKind::Return)
            {
                edges.push(EdgeToAdd {
                    source: idx,
                    target: site.stmt_idx,
                    weight: CpgEdge::DataFlowReturn,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tree-sitter call extraction
// ---------------------------------------------------------------------------

/// Find a tree-sitter node at the given byte range (shallowest match).
fn find_node_at_bytes(
    root: tree_sitter::Node,
    start_byte: usize,
    end_byte: usize,
) -> Option<tree_sitter::Node> {
    if root.start_byte() == start_byte && root.end_byte() == end_byte {
        return Some(root);
    }
    let mut cursor = root.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if child.start_byte() <= start_byte && child.end_byte() >= end_byte {
                if let Some(found) = find_node_at_bytes(child, start_byte, end_byte) {
                    return Some(found);
                }
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    None
}

/// Recursively extract call nodes from a tree-sitter subtree.
fn extract_calls_from_node(
    node: tree_sitter::Node,
    source: &str,
    stmt_idx: NodeIndex,
    func_idx: NodeIndex,
    sites: &mut Vec<CallSite>,
) {
    if node.kind() == "call" {
        if let Some(site) = parse_call_node(node, source, stmt_idx, func_idx) {
            sites.push(site);
        }
    }

    // Recurse into children
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            extract_calls_from_node(cursor.node(), source, stmt_idx, func_idx, sites);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

/// Parse a tree-sitter `call` node into a CallSite.
fn parse_call_node(
    call_node: tree_sitter::Node,
    source: &str,
    stmt_idx: NodeIndex,
    func_idx: NodeIndex,
) -> Option<CallSite> {
    let function_node = call_node.child_by_field_name("function")?;

    let (callee_name, receiver) = match function_node.kind() {
        "identifier" => {
            let name = function_node.utf8_text(source.as_bytes()).ok()?.to_string();
            (name, None)
        }
        "attribute" => {
            let attr_name = function_node
                .child_by_field_name("attribute")?
                .utf8_text(source.as_bytes())
                .ok()?
                .to_string();
            let object = function_node
                .child_by_field_name("object")?
                .utf8_text(source.as_bytes())
                .ok()?
                .to_string();
            (attr_name, Some(object))
        }
        _ => return None,
    };

    let arguments_node = call_node.child_by_field_name("arguments")?;
    let (positional_args, keyword_args) = extract_arguments(arguments_node, source);

    Some(CallSite {
        stmt_idx,
        caller_func_idx: func_idx,
        callee_name,
        receiver,
        positional_args,
        keyword_args,
        call_start_byte: call_node.start_byte(),
        call_end_byte: call_node.end_byte(),
    })
}

/// Extract positional and keyword arguments from an argument_list node.
fn extract_arguments(
    args_node: tree_sitter::Node,
    source: &str,
) -> (Vec<String>, Vec<(String, String)>) {
    let mut positional = Vec::new();
    let mut keyword = Vec::new();

    let mut cursor = args_node.walk();
    for child in args_node.named_children(&mut cursor) {
        match child.kind() {
            "keyword_argument" => {
                let name = child
                    .child_by_field_name("name")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .unwrap_or("")
                    .to_string();
                let value = child
                    .child_by_field_name("value")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .unwrap_or("")
                    .to_string();
                keyword.push((name, value));
            }
            "list_splat" | "dictionary_splat" => {
                // *args, **kwargs — skip for now
            }
            _ => {
                if let Ok(text) = child.utf8_text(source.as_bytes()) {
                    positional.push(text.to_string());
                }
            }
        }
    }

    (positional, keyword)
}
