use petgraph::graph::NodeIndex;
use std::path::Path;

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

/// Stateless builder for call graph construction.
pub struct CallGraphBuilder;

impl CallGraphBuilder {
    // -----------------------------------------------------------------------
    // Pass 1: Extract call sites from a single function
    // -----------------------------------------------------------------------

    /// Extract all call sites from statements within a function.
    /// Called during `build_file` after CFG and dataflow analysis.
    pub fn extract_call_sites(cpg: &CpgLayer, func_idx: NodeIndex, source: &str) -> Vec<CallSite> {
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
                    if let Some(ts_node) =
                        find_node_at_bytes(tree.root_node(), node.start_byte, node.end_byte)
                    {
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

    /// Resolve CPG edges through the same AST-only binding engine used by
    /// repository callers/callees queries. A complete pass over the lazily loaded
    /// CPG files makes full, incremental, and query-order paths equivalent.
    pub fn resolve_all(cpg: &mut CpgLayer, _symbol_index: &SymbolIndex) {
        let mut index = crate::call_index::CallIndex::new();
        for (path, tree) in &cpg.trees {
            if let Some(source) = cpg.sources.get(path) {
                index.update_file(
                    path,
                    tree,
                    source,
                    crate::parser::SupportedLanguage::from_path(path),
                    cpg.import_bindings.get(path).cloned().unwrap_or_default(),
                );
            }
        }
        index.resolve_all();
        let mut nodes: std::collections::HashMap<_, _> = cpg
            .graph
            .node_indices()
            .filter_map(|idx| {
                let node = &cpg.graph[idx];
                matches!(node.kind, CpgNodeKind::Function | CpgNodeKind::Method)
                    .then(|| ((node.file_path.clone(), node.start_byte), idx))
            })
            .collect();
        // JS arrow declarations use the enclosing variable statement in the
        // detailed CPG and the function expression in the lightweight index.
        // Associate by qualified declaration and containment, not node index.
        for (path, indices) in &cpg.file_to_nodes {
            for declaration in index.functions(path) {
                let candidates = indices.iter().filter(|&&idx| {
                    let node = &cpg.graph[idx];
                    let qualified = node
                        .parent_class
                        .as_ref()
                        .map(|class| format!("{}.{}", class, node.name))
                        .unwrap_or_else(|| node.name.clone());
                    matches!(node.kind, CpgNodeKind::Function | CpgNodeKind::Method)
                        && qualified == declaration.qualified_name
                        && node.start_byte <= declaration.start_byte
                        && node.end_byte >= declaration.end_byte
                });
                if let Some(&idx) = candidates
                    .min_by_key(|&&idx| cpg.graph[idx].end_byte - cpg.graph[idx].start_byte)
                {
                    nodes.insert((path.clone(), declaration.start_byte), idx);
                }
            }
        }
        let mut edges_to_add = Vec::new();
        for sites in cpg.call_sites.values() {
            for site in sites {
                let Some(caller) = cpg.graph.node_weight(site.caller_func_idx) else {
                    continue;
                };
                if let Some(target) = index.resolved_target(&caller.file_path, site.call_start_byte)
                {
                    let owner = index.call_owner(&caller.file_path, site.call_start_byte);
                    if owner.and_then(|id| nodes.get(&(id.file_path.clone(), id.start_byte)))
                        != Some(&site.caller_func_idx)
                    {
                        continue;
                    }
                    if let Some(&callee) = nodes.get(&(target.file_path.clone(), target.start_byte))
                    {
                        collect_edges_for_resolved_call(cpg, site, callee, &mut edges_to_add);
                    }
                }
            }
        }
        // Clear every interprocedural edge as one batch. Partial removal of an
        // affected caller erased unrelated calls and incoming caller edges.
        let mut remove: Vec<_> = cpg
            .graph
            .edge_indices()
            .filter(|&id| is_interprocedural_edge(&cpg.graph[id]))
            .collect();
        remove.sort_by_key(|id| std::cmp::Reverse(id.index()));
        for id in remove {
            cpg.graph.remove_edge(id);
        }
        for edge in edges_to_add {
            if !cpg
                .graph
                .edges_connecting(edge.source, edge.target)
                .any(|e| *e.weight() == edge.weight)
            {
                cpg.graph.add_edge(edge.source, edge.target, edge.weight);
            }
        }
    }

    pub fn resolve_file(cpg: &mut CpgLayer, _file_path: &Path, symbol_index: &SymbolIndex) {
        Self::resolve_all(cpg, symbol_index);
    }

    pub fn resolve_new_callers(cpg: &mut CpgLayer, _file_path: &Path, symbol_index: &SymbolIndex) {
        Self::resolve_all(cpg, symbol_index);
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
            .filter(|(_, p)| p.name != "self" && p.name != "cls" && !p.name.starts_with('*'))
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
    if node.kind() == "call" || node.kind() == "call_expression" {
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
            // Python: obj.method
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
        "member_expression" => {
            // JS/TS: obj.method
            let prop_name = function_node
                .child_by_field_name("property")?
                .utf8_text(source.as_bytes())
                .ok()?
                .to_string();
            let object = function_node
                .child_by_field_name("object")?
                .utf8_text(source.as_bytes())
                .ok()?
                .to_string();
            (prop_name, Some(object))
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
            "list_splat" | "dictionary_splat" | "spread_element" => {
                // *args, **kwargs, ...args — skip for now
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
