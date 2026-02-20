use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::cpg::{CpgEdge, CpgLayer, CpgNodeKind, StatementKind};

/// Extracted def/use info for a single statement.
struct StmtDefUse {
    defs: Vec<String>,
    uses: Vec<String>,
}

/// A definition: (variable_name, defining_statement_index).
type Def = (String, NodeIndex);

/// The main data flow analyzer. All methods are associated functions (stateless).
pub struct DataFlowAnalyzer;

impl DataFlowAnalyzer {
    /// Main entry point: run reaching definitions analysis for a single function.
    pub fn analyze_function(cpg: &mut CpgLayer, func_idx: NodeIndex, source: &str) {
        let file_path = match cpg.graph.node_weight(func_idx) {
            Some(node) => node.file_path.clone(),
            None => return,
        };

        // Collect all CFG nodes belonging to this function
        let entry_idx = match cpg.function_to_entry.get(&func_idx) {
            Some(&idx) => idx,
            None => return,
        };
        let exit_idx = match cpg.function_to_exit.get(&func_idx) {
            Some(&idx) => idx,
            None => return,
        };

        let cfg_nodes: Vec<NodeIndex> = cpg.get_stmts_for_function(func_idx).to_vec();

        // Get the persisted tree for this file
        let tree = match cpg.trees.get(&file_path) {
            Some(t) => t.clone(),
            None => return,
        };

        // Extract defs/uses for each CFG node
        for &node_idx in &cfg_nodes {
            let du = extract_def_use(cpg, node_idx, &tree, source, func_idx);
            cpg.stmt_defs.insert(node_idx, du.defs);
            cpg.stmt_uses.insert(node_idx, du.uses);
        }

        // Build the set of all variable names defined anywhere in this function
        let all_defs: HashMap<String, Vec<NodeIndex>> = {
            let mut map: HashMap<String, Vec<NodeIndex>> = HashMap::new();
            for &node_idx in &cfg_nodes {
                if let Some(defs) = cpg.stmt_defs.get(&node_idx) {
                    for var in defs {
                        map.entry(var.clone()).or_default().push(node_idx);
                    }
                }
            }
            map
        };

        // GEN and KILL sets per node
        let mut gen_sets: HashMap<NodeIndex, HashSet<Def>> = HashMap::new();
        let mut kill: HashMap<NodeIndex, HashSet<Def>> = HashMap::new();

        for &node_idx in &cfg_nodes {
            let mut node_gen: HashSet<Def> = HashSet::new();
            let mut kill_set: HashSet<Def> = HashSet::new();

            if let Some(defs) = cpg.stmt_defs.get(&node_idx) {
                for var in defs {
                    node_gen.insert((var.clone(), node_idx));
                    // KILL: all other definitions of the same variable
                    if let Some(other_defs) = all_defs.get(var) {
                        for &other_idx in other_defs {
                            if other_idx != node_idx {
                                kill_set.insert((var.clone(), other_idx));
                            }
                        }
                    }
                }
            }

            gen_sets.insert(node_idx, node_gen);
            kill.insert(node_idx, kill_set);
        }

        // Worklist algorithm: compute IN/OUT sets
        let mut in_sets: HashMap<NodeIndex, HashSet<Def>> = HashMap::new();
        let mut out_sets: HashMap<NodeIndex, HashSet<Def>> = HashMap::new();
        for &node_idx in &cfg_nodes {
            in_sets.insert(node_idx, HashSet::new());
            out_sets.insert(node_idx, HashSet::new());
        }

        // Build predecessor map from CFG edges
        let cfg_node_set: HashSet<NodeIndex> = cfg_nodes.iter().copied().collect();
        let mut predecessors: HashMap<NodeIndex, Vec<NodeIndex>> = HashMap::new();
        for &node_idx in &cfg_nodes {
            predecessors.insert(node_idx, Vec::new());
        }
        for &node_idx in &cfg_nodes {
            for edge in cpg.graph.edges_directed(node_idx, petgraph::Direction::Outgoing) {
                let target = edge.target();
                if cfg_node_set.contains(&target) {
                    match edge.weight() {
                        CpgEdge::ControlFlowNext
                        | CpgEdge::ControlFlowTrue
                        | CpgEdge::ControlFlowFalse
                        | CpgEdge::ControlFlowException
                        | CpgEdge::ControlFlowBack => {
                            predecessors.entry(target).or_default().push(node_idx);
                        }
                        _ => {}
                    }
                }
            }
        }

        // Initialize worklist with all nodes
        let mut worklist: VecDeque<NodeIndex> = cfg_nodes.iter().copied().collect();

        // Safety cap: reaching definitions converges in O(nodes * variables) iterations.
        // This limit prevents hangs from pathological CFG shapes or bugs.
        const MAX_ITERATIONS: usize = 100_000;
        let mut iterations: usize = 0;

        while let Some(node_idx) = worklist.pop_front() {
            iterations += 1;
            if iterations > MAX_ITERATIONS {
                break;
            }
            // IN(node) = union of OUT(pred) for all predecessors
            let mut new_in: HashSet<Def> = HashSet::new();
            if let Some(preds) = predecessors.get(&node_idx) {
                for &pred in preds {
                    if let Some(pred_out) = out_sets.get(&pred) {
                        new_in.extend(pred_out.iter().cloned());
                    }
                }
            }
            in_sets.insert(node_idx, new_in.clone());

            // OUT(node) = GEN(node) ∪ (IN(node) - KILL(node))
            let node_gen = gen_sets.get(&node_idx).cloned().unwrap_or_default();
            let kill_set = kill.get(&node_idx).cloned().unwrap_or_default();
            let new_out: HashSet<Def> = node_gen
                .union(&new_in.difference(&kill_set).cloned().collect())
                .cloned()
                .collect();

            let old_out = out_sets.get(&node_idx).cloned().unwrap_or_default();
            if new_out != old_out {
                out_sets.insert(node_idx, new_out);
                // Add successors to worklist
                for edge in cpg.graph.edges_directed(node_idx, petgraph::Direction::Outgoing) {
                    let target = edge.target();
                    if cfg_node_set.contains(&target) {
                        match edge.weight() {
                            CpgEdge::ControlFlowNext
                            | CpgEdge::ControlFlowTrue
                            | CpgEdge::ControlFlowFalse
                            | CpgEdge::ControlFlowException
                            | CpgEdge::ControlFlowBack => {
                                worklist.push_back(target);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Add DataFlowReach edges: for each use at node U, find reaching definitions in IN(U)
        let mut edges_to_add: Vec<(NodeIndex, NodeIndex, String)> = Vec::new();
        for &node_idx in &cfg_nodes {
            if let Some(uses) = cpg.stmt_uses.get(&node_idx) {
                if let Some(in_set) = in_sets.get(&node_idx) {
                    for var in uses {
                        for (def_var, def_node) in in_set {
                            if def_var == var {
                                edges_to_add.push((*def_node, node_idx, var.clone()));
                            }
                        }
                    }
                }
            }
        }

        for (def_node, use_node, var_name) in edges_to_add {
            cpg.graph.add_edge(def_node, use_node, CpgEdge::DataFlowReach(var_name));
        }
    }
}

// ---------------------------------------------------------------------------
// Def/use extraction
// ---------------------------------------------------------------------------

/// Extract defs and uses for a single CFG node.
fn extract_def_use(
    cpg: &CpgLayer,
    node_idx: NodeIndex,
    tree: &tree_sitter::Tree,
    source: &str,
    func_idx: NodeIndex,
) -> StmtDefUse {
    let node = match cpg.graph.node_weight(node_idx) {
        Some(n) => n,
        None => return StmtDefUse { defs: vec![], uses: vec![] },
    };

    match node.kind {
        CpgNodeKind::CfgEntry => {
            // Defs = function parameters (excluding self/cls)
            let func_node = match cpg.graph.node_weight(func_idx) {
                Some(n) => n,
                None => return StmtDefUse { defs: vec![], uses: vec![] },
            };
            let defs: Vec<String> = func_node
                .parameters
                .iter()
                .map(|p| p.name.clone())
                .filter(|name| name != "self" && name != "cls")
                // Filter out *args/**kwargs prefixes for clean names
                .map(|name| name.trim_start_matches('*').to_string())
                .filter(|name| !name.is_empty())
                .collect();
            StmtDefUse { defs, uses: vec![] }
        }
        CpgNodeKind::CfgExit => StmtDefUse { defs: vec![], uses: vec![] },
        CpgNodeKind::Statement => {
            // Find the tree-sitter node by byte range (deepest match)
            let ts_node = match find_deepest_node_at_bytes(
                tree.root_node(),
                node.start_byte,
                node.end_byte,
            ) {
                Some(n) => n,
                None => return StmtDefUse { defs: vec![], uses: vec![] },
            };

            // Dispatch based on the actual tree-sitter node kind, since
            // find_ts_node_at_bytes may return a deeper node (e.g. assignment
            // inside expression_statement when they share the same byte range).
            dispatch_def_use(ts_node, source, node.statement_kind.as_ref())
        }
        _ => StmtDefUse { defs: vec![], uses: vec![] },
    }
}

/// Dispatch def/use extraction based on the actual tree-sitter node kind.
/// Falls back to the stored StatementKind when ts_node kind is ambiguous.
fn dispatch_def_use(ts_node: tree_sitter::Node, source: &str, stmt_kind: Option<&StatementKind>) -> StmtDefUse {
    // Primary dispatch: use the actual tree-sitter node kind
    match ts_node.kind() {
        "assignment" | "augmented_assignment" => extract_assignment_def_use(ts_node, source),
        "for_statement" => extract_for_def_use(ts_node, source),
        "with_statement" => extract_with_def_use(ts_node, source),
        "if_statement" | "while_statement" | "elif_clause" => extract_if_while_def_use(ts_node, source),
        "import_statement" | "import_from_statement" => extract_import_def_use(ts_node, source),
        "expression_statement" => extract_expression_statement_def_use(ts_node, source),
        _ => {
            // Secondary dispatch: use stored statement kind
            match stmt_kind {
                Some(StatementKind::Assignment) => extract_assignment_def_use(ts_node, source),
                Some(StatementKind::For) => extract_for_def_use(ts_node, source),
                Some(StatementKind::With) => extract_with_def_use(ts_node, source),
                Some(StatementKind::If) | Some(StatementKind::While) => {
                    extract_if_while_def_use(ts_node, source)
                }
                Some(StatementKind::Import) => extract_import_def_use(ts_node, source),
                Some(StatementKind::ExpressionStatement) => {
                    extract_expression_statement_def_use(ts_node, source)
                }
                _ => {
                    // return, assert, pass, break, continue, raise, delete, other:
                    // no defs, all identifiers are uses
                    let mut uses = Vec::new();
                    collect_expression_identifiers(ts_node, source, &mut uses);
                    StmtDefUse { defs: vec![], uses }
                }
            }
        }
    }
}

/// Extract defs/uses from an assignment or augmented assignment.
fn extract_assignment_def_use(ts_node: tree_sitter::Node, source: &str) -> StmtDefUse {
    let mut defs = Vec::new();
    let mut uses = Vec::new();

    match ts_node.kind() {
        "assignment" => {
            if let Some(left) = ts_node.child_by_field_name("left") {
                collect_target_identifiers(left, source, &mut defs);
            }
            if let Some(right) = ts_node.child_by_field_name("right") {
                collect_expression_identifiers(right, source, &mut uses);
            }
            // Also check for type annotation
            if let Some(type_node) = ts_node.child_by_field_name("type") {
                collect_expression_identifiers(type_node, source, &mut uses);
            }
        }
        "augmented_assignment" => {
            // x += y  →  defs: {x}, uses: {x, y}
            if let Some(left) = ts_node.child_by_field_name("left") {
                collect_target_identifiers(left, source, &mut defs);
                // Augmented assignment also reads the target
                collect_expression_identifiers(left, source, &mut uses);
            }
            if let Some(right) = ts_node.child_by_field_name("right") {
                collect_expression_identifiers(right, source, &mut uses);
            }
        }
        _ => {}
    }

    StmtDefUse { defs, uses }
}

/// Extract defs/uses from an expression_statement (may wrap an assignment).
fn extract_expression_statement_def_use(ts_node: tree_sitter::Node, source: &str) -> StmtDefUse {
    // find_ts_node_at_bytes may return the inner assignment directly (same byte range)
    match ts_node.kind() {
        "assignment" | "augmented_assignment" => {
            return extract_assignment_def_use(ts_node, source);
        }
        _ => {}
    }
    // Check if this wraps an assignment or augmented_assignment
    if let Some(child) = ts_node.named_child(0) {
        match child.kind() {
            "assignment" | "augmented_assignment" => {
                return extract_assignment_def_use(child, source);
            }
            _ => {}
        }
    }
    // Otherwise, it's a bare expression: no defs, all identifiers are uses
    let mut uses = Vec::new();
    collect_expression_identifiers(ts_node, source, &mut uses);
    StmtDefUse { defs: vec![], uses }
}

/// Extract defs/uses from a for statement header.
fn extract_for_def_use(ts_node: tree_sitter::Node, source: &str) -> StmtDefUse {
    let mut defs = Vec::new();
    let mut uses = Vec::new();

    // defs from the loop variable (left field)
    if let Some(left) = ts_node.child_by_field_name("left") {
        collect_target_identifiers(left, source, &mut defs);
    }
    // uses from the iterable (right field)
    if let Some(right) = ts_node.child_by_field_name("right") {
        collect_expression_identifiers(right, source, &mut uses);
    }

    StmtDefUse { defs, uses }
}

/// Extract defs/uses from a with statement header.
/// Tree structure: with_statement → with_clause → with_item → as_pattern
///   as_pattern contains: expression (uses) + as_pattern_target (defs)
fn extract_with_def_use(ts_node: tree_sitter::Node, source: &str) -> StmtDefUse {
    let mut defs = Vec::new();
    let mut uses = Vec::new();

    // Recursively find all as_pattern_target nodes (defs) and
    // context expression nodes (uses) within with_clause/with_item
    collect_with_defs_uses(ts_node, source, &mut defs, &mut uses);

    StmtDefUse { defs, uses }
}

/// Recursively extract defs/uses from with statement subtree.
fn collect_with_defs_uses(
    node: tree_sitter::Node,
    source: &str,
    defs: &mut Vec<String>,
    uses: &mut Vec<String>,
) {
    match node.kind() {
        "as_pattern_target" => {
            // This is the `h` in `with open(f) as h:`
            collect_target_identifiers(node, source, defs);
        }
        "as_pattern" => {
            // Children: expression, 'as' keyword, as_pattern_target
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "as_pattern_target" {
                    collect_target_identifiers(child, source, defs);
                } else {
                    // The expression part (context manager) — collect as uses
                    collect_expression_identifiers(child, source, uses);
                }
            }
        }
        "with_clause" | "with_item" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_with_defs_uses(child, source, defs, uses);
            }
        }
        // Don't descend into the body block
        "block" => {}
        _ => {
            // For with_statement itself, recurse into named children
            // but skip the body block and keywords
            if node.kind() == "with_statement" {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    if child.kind() != "block" {
                        collect_with_defs_uses(child, source, defs, uses);
                    }
                }
            }
        }
    }
}

/// Extract defs/uses from if/while condition (no defs, uses from condition only).
fn extract_if_while_def_use(ts_node: tree_sitter::Node, source: &str) -> StmtDefUse {
    let mut uses = Vec::new();
    if let Some(condition) = ts_node.child_by_field_name("condition") {
        collect_expression_identifiers(condition, source, &mut uses);
    }
    StmtDefUse { defs: vec![], uses }
}

/// Extract defs from import statements.
fn extract_import_def_use(ts_node: tree_sitter::Node, source: &str) -> StmtDefUse {
    let mut defs = Vec::new();

    match ts_node.kind() {
        "import_statement" => {
            // import foo, bar → defs: {foo, bar}
            let mut cursor = ts_node.walk();
            for child in ts_node.named_children(&mut cursor) {
                match child.kind() {
                    "dotted_name" => {
                        // Take first component as the local name
                        if let Some(first) = child.named_child(0) {
                            if let Ok(text) = first.utf8_text(source.as_bytes()) {
                                defs.push(text.to_string());
                            }
                        }
                    }
                    "aliased_import" => {
                        // import foo as bar → defs: {bar}
                        if let Some(alias) = child.child_by_field_name("alias") {
                            if let Ok(text) = alias.utf8_text(source.as_bytes()) {
                                defs.push(text.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        "import_from_statement" => {
            // from foo import bar, baz → defs: {bar, baz}
            let mut cursor = ts_node.walk();
            for child in ts_node.named_children(&mut cursor) {
                match child.kind() {
                    "dotted_name" | "identifier" => {
                        // Skip the module name (before "import" keyword)
                        // Only include names that come after "import"
                    }
                    "aliased_import" => {
                        if let Some(alias) = child.child_by_field_name("alias") {
                            if let Ok(text) = alias.utf8_text(source.as_bytes()) {
                                defs.push(text.to_string());
                            }
                        } else if let Some(name) = child.child_by_field_name("name") {
                            if let Ok(text) = name.utf8_text(source.as_bytes()) {
                                defs.push(text.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            // For `from X import name1, name2` — find imported names
            if let Some(names_node) = find_import_names(ts_node) {
                let mut cursor2 = names_node.walk();
                for child in names_node.children(&mut cursor2) {
                    if child.is_named() {
                        match child.kind() {
                            "dotted_name" | "identifier" => {
                                if let Ok(text) = child.utf8_text(source.as_bytes()) {
                                    if !defs.contains(&text.to_string()) {
                                        defs.push(text.to_string());
                                    }
                                }
                            }
                            "aliased_import" => {
                                // Already handled above
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        _ => {}
    }

    StmtDefUse { defs, uses: vec![] }
}

/// Helper: find the import names section of a from-import statement.
fn find_import_names(ts_node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    // In `from X import a, b`, the imported names are after the module_name field
    // tree-sitter-python uses "name" children after the "import" keyword
    // We need to look for identifiers that are not the module name
    let mut cursor = ts_node.walk();
    let mut past_import = false;

    for child in ts_node.children(&mut cursor) {
        if child.kind() == "import" {
            past_import = true;
            continue;
        }
        if past_import && child.is_named() {
            // This could be a single name or the start of a name list
            return None; // Let the parent handle individual names
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tree-sitter node lookup
// ---------------------------------------------------------------------------

/// Find the deepest tree-sitter node matching the given byte range.
/// Unlike cfg::find_ts_node_at_bytes which returns the shallowest match,
/// this keeps descending into children that also match the same byte range
/// (e.g. block → expression_statement → assignment all sharing same bytes).
fn find_deepest_node_at_bytes(root: tree_sitter::Node, start_byte: usize, end_byte: usize) -> Option<tree_sitter::Node> {
    if root.start_byte() == start_byte && root.end_byte() == end_byte {
        // Found a match, but try to find a deeper one
        let mut cursor = root.walk();
        if cursor.goto_first_child() {
            loop {
                let child = cursor.node();
                if child.start_byte() == start_byte && child.end_byte() == end_byte {
                    // Child also matches — recurse deeper
                    return find_deepest_node_at_bytes(child, start_byte, end_byte);
                }
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
        return Some(root);
    }

    // Not a match at this level — look in children
    let mut cursor = root.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if child.start_byte() <= start_byte && child.end_byte() >= end_byte {
                if let Some(found) = find_deepest_node_at_bytes(child, start_byte, end_byte) {
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

// ---------------------------------------------------------------------------
// Tree-sitter walker helpers
// ---------------------------------------------------------------------------

/// Walk the LHS of an assignment, collecting bare identifier names that
/// represent local variable definitions. Handles tuple unpacking.
/// Skips attribute access (self.x) and subscript (a[0]).
fn collect_target_identifiers(node: tree_sitter::Node, source: &str, out: &mut Vec<String>) {
    match node.kind() {
        "identifier" => {
            if let Ok(text) = node.utf8_text(source.as_bytes()) {
                let name = text.to_string();
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        "pattern_list" | "tuple_pattern" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_target_identifiers(child, source, out);
            }
        }
        "starred_expression" => {
            // *rest in tuple unpacking
            if let Some(child) = node.named_child(0) {
                collect_target_identifiers(child, source, out);
            }
        }
        "list_pattern" | "as_pattern_target" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_target_identifiers(child, source, out);
            }
        }
        // Skip attribute, subscript — not local variable defs
        "attribute" | "subscript" => {}
        _ => {}
    }
}

/// Walk an expression subtree collecting identifier names (uses).
/// Filters out self, cls, True, False, None.
/// For attribute nodes, only collects the root object (not .attr_name).
/// Skips string/number/keyword literals.
fn collect_expression_identifiers(node: tree_sitter::Node, source: &str, out: &mut Vec<String>) {
    match node.kind() {
        "identifier" => {
            if let Ok(text) = node.utf8_text(source.as_bytes()) {
                let name = text.to_string();
                // Filter out special names
                if name != "self"
                    && name != "cls"
                    && name != "True"
                    && name != "False"
                    && name != "None"
                    && !out.contains(&name)
                {
                    out.push(name);
                }
            }
        }
        "attribute" => {
            // For `obj.attr`, only collect `obj` (the root object), not `attr`
            if let Some(object) = node.child_by_field_name("object") {
                collect_expression_identifiers(object, source, out);
            }
        }
        "string" | "integer" | "float" | "true" | "false" | "none"
        | "concatenated_string" | "string_content" | "escape_sequence" => {
            // Skip literals
        }
        "comment" => {}
        // For function/class definitions nested inside (shouldn't happen in statements, but be safe)
        "function_definition" | "async_function_definition" | "class_definition" => {}
        "lambda" => {
            // Don't descend into lambda bodies for now
        }
        _ => {
            // Recurse into children
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_expression_identifiers(child, source, out);
            }
        }
    }
}
