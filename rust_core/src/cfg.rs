use petgraph::graph::NodeIndex;
use std::path::PathBuf;

use crate::cpg::{CpgEdge, CpgLayer, CpgNode, CpgNodeKind, StatementKind};
use crate::parser::SupportedLanguage;

/// Result of building CFG for a statement or block.
struct CfgBuildResult {
    /// First node executed in this block/statement.
    entry: NodeIndex,
    /// All possible exit points (multiple for branches).
    exits: Vec<NodeIndex>,
}

/// Loop context for break/continue targeting.
struct LoopContext {
    /// continue jumps here (loop header).
    header_idx: NodeIndex,
    /// break nodes collected as additional loop exits.
    break_collectors: Vec<NodeIndex>,
}

/// Build context threaded through recursive CFG construction.
struct BuildContext {
    /// The function node this CFG belongs to.
    func_idx: NodeIndex,
    /// The CfgExit sentinel for the owning function.
    exit_idx: NodeIndex,
    /// Stack of enclosing loops (innermost last).
    loop_stack: Vec<LoopContext>,
    /// File path for new nodes.
    file_path: PathBuf,
}

/// Builds intra-procedural control flow graphs for functions.
pub struct CfgBuilder {
    language: SupportedLanguage,
}

impl CfgBuilder {
    pub fn new(language: SupportedLanguage) -> Self {
        Self { language }
    }

    /// Main entry point: build CFG for a single function/method.
    pub fn build_function_cfg(
        &self,
        cpg: &mut CpgLayer,
        func_idx: NodeIndex,
        source: &str,
    ) -> Option<()> {
        let func_node = cpg.graph.node_weight(func_idx)?;
        let file_path = func_node.file_path.clone();
        let start_byte = func_node.start_byte;
        let end_byte = func_node.end_byte;
        let start_line = func_node.start_line;
        let end_line = func_node.end_line;

        // Create CfgEntry sentinel
        let entry_node = CpgNode {
            file_path: file_path.clone(),
            name: "<entry>".to_string(),
            kind: CpgNodeKind::CfgEntry,
            start_byte,
            end_byte: start_byte,
            start_line,
            end_line: start_line,
            parameters: Vec::new(),
            return_type: None,
            docstring: None,
            bases: Vec::new(),
            parent_class: None,
            statement_kind: None,
            function_idx: Some(func_idx),
        };
        let entry_idx = cpg.graph.add_node(entry_node);
        cpg.function_to_stmts.entry(func_idx).or_default().push(entry_idx);

        // Create CfgExit sentinel
        let exit_node = CpgNode {
            file_path: file_path.clone(),
            name: "<exit>".to_string(),
            kind: CpgNodeKind::CfgExit,
            start_byte: end_byte,
            end_byte,
            start_line: end_line,
            end_line,
            parameters: Vec::new(),
            return_type: None,
            docstring: None,
            bases: Vec::new(),
            parent_class: None,
            statement_kind: None,
            function_idx: Some(func_idx),
        };
        let exit_idx = cpg.graph.add_node(exit_node);
        cpg.function_to_stmts.entry(func_idx).or_default().push(exit_idx);

        // Register in maps
        cpg.function_to_entry.insert(func_idx, entry_idx);
        cpg.function_to_exit.insert(func_idx, exit_idx);

        // Also add entry/exit to file_to_nodes
        if let Some(node_list) = cpg.file_to_nodes.get_mut(&file_path) {
            node_list.push(entry_idx);
            node_list.push(exit_idx);
        }

        // Find the function's tree-sitter node in the persisted tree
        let tree = cpg.trees.get(&file_path)?.clone();
        let mut func_ts_node = find_ts_node_at_bytes(
            tree.root_node(),
            start_byte,
            end_byte,
        )?;

        // When a class body block shares the same byte range as its sole
        // function child, find_ts_node_at_bytes returns the block.
        // Descend into it to find the actual function definition.
        if func_ts_node.kind() == "block" || func_ts_node.kind() == "class_body" {
            let mut cursor = func_ts_node.walk();
            for child in func_ts_node.named_children(&mut cursor) {
                if matches!(child.kind(),
                    "function_definition" | "async_function_definition"
                    | "function_declaration" | "generator_function_declaration"
                    | "method_definition"
                ) && child.start_byte() == start_byte
                    && child.end_byte() == end_byte
                {
                    func_ts_node = child;
                    break;
                }
            }
        }

        // Get the body block
        let body = func_ts_node.child_by_field_name("body")?;

        let mut ctx = BuildContext {
            func_idx,
            exit_idx,
            loop_stack: Vec::new(),
            file_path,
        };

        // Build CFG for the function body
        if let Some(result) = self.build_block_cfg(cpg, body, &mut ctx, source) {
            // Connect entry → first statement
            cpg.graph.add_edge(entry_idx, result.entry, CpgEdge::ControlFlowNext);
            // Connect all exits → CfgExit
            for exit in result.exits {
                cpg.graph.add_edge(exit, exit_idx, CpgEdge::ControlFlowNext);
            }
        } else {
            // Empty body: entry → exit directly
            cpg.graph.add_edge(entry_idx, exit_idx, CpgEdge::ControlFlowNext);
        }

        Some(())
    }

    /// Build CFG for a block (sequence of statements).
    fn build_block_cfg(
        &self,
        cpg: &mut CpgLayer,
        body_node: tree_sitter::Node,
        ctx: &mut BuildContext,
        source: &str,
    ) -> Option<CfgBuildResult> {
        let mut cursor = body_node.walk();
        let children: Vec<_> = body_node.named_children(&mut cursor).collect();

        if children.is_empty() {
            return None;
        }

        let mut first_entry: Option<NodeIndex> = None;
        let mut prev_exits: Vec<NodeIndex> = Vec::new();

        for child in children {
            if let Some(result) = self.build_statement_cfg(cpg, child, ctx, source) {
                if first_entry.is_none() {
                    first_entry = Some(result.entry);
                }

                // Chain: previous exits → this entry
                for prev_exit in &prev_exits {
                    cpg.graph.add_edge(*prev_exit, result.entry, CpgEdge::ControlFlowNext);
                }

                prev_exits = result.exits;
            }
        }

        first_entry.map(|entry| CfgBuildResult {
            entry,
            exits: prev_exits,
        })
    }

    /// Build CFG for a single statement. Dispatches based on statement kind.
    fn build_statement_cfg(
        &self,
        cpg: &mut CpgLayer,
        ts_node: tree_sitter::Node,
        ctx: &mut BuildContext,
        source: &str,
    ) -> Option<CfgBuildResult> {
        let kind = classify_statement(ts_node);

        match kind {
            StatementKind::If => self.build_if_cfg(cpg, ts_node, ctx, source),
            StatementKind::For => self.build_for_cfg(cpg, ts_node, ctx, source),
            StatementKind::While => self.build_while_cfg(cpg, ts_node, ctx, source),
            StatementKind::Try => self.build_try_cfg(cpg, ts_node, ctx, source),
            StatementKind::With => self.build_with_cfg(cpg, ts_node, ctx, source),
            StatementKind::Match => self.build_match_cfg(cpg, ts_node, ctx, source),
            StatementKind::Return | StatementKind::Raise => {
                let stmt_idx = self.create_statement_node(cpg, ts_node, ctx, source, kind);
                // Jump directly to function exit
                cpg.graph.add_edge(stmt_idx, ctx.exit_idx, CpgEdge::ControlFlowNext);
                Some(CfgBuildResult {
                    entry: stmt_idx,
                    exits: Vec::new(), // Terminates this path
                })
            }
            StatementKind::Break => {
                let stmt_idx = self.create_statement_node(cpg, ts_node, ctx, source, kind);
                // Push to loop's break collectors
                if let Some(loop_ctx) = ctx.loop_stack.last_mut() {
                    loop_ctx.break_collectors.push(stmt_idx);
                }
                Some(CfgBuildResult {
                    entry: stmt_idx,
                    exits: Vec::new(), // Terminates this path within loop
                })
            }
            StatementKind::Continue => {
                let stmt_idx = self.create_statement_node(cpg, ts_node, ctx, source, kind);
                // Back-edge to loop header
                if let Some(loop_ctx) = ctx.loop_stack.last() {
                    cpg.graph.add_edge(stmt_idx, loop_ctx.header_idx, CpgEdge::ControlFlowBack);
                }
                Some(CfgBuildResult {
                    entry: stmt_idx,
                    exits: Vec::new(), // Terminates this path within loop
                })
            }
            _ => {
                // Simple statement: single node, passes through
                let stmt_idx = self.create_statement_node(cpg, ts_node, ctx, source, kind);
                Some(CfgBuildResult {
                    entry: stmt_idx,
                    exits: vec![stmt_idx],
                })
            }
        }
    }

    /// Build CFG for if/elif/else.
    fn build_if_cfg(
        &self,
        cpg: &mut CpgLayer,
        ts_node: tree_sitter::Node,
        ctx: &mut BuildContext,
        source: &str,
    ) -> Option<CfgBuildResult> {
        let stmt_idx = self.create_statement_node(cpg, ts_node, ctx, source, StatementKind::If);
        let mut all_exits: Vec<NodeIndex> = Vec::new();
        let mut has_else = false;

        // consequence (true branch)
        if let Some(consequence) = ts_node.child_by_field_name("consequence") {
            if let Some(result) = self.build_block_cfg(cpg, consequence, ctx, source) {
                cpg.graph.add_edge(stmt_idx, result.entry, CpgEdge::ControlFlowTrue);
                all_exits.extend(result.exits);
            } else {
                // Empty consequence: if_stmt itself is an exit of this branch
                all_exits.push(stmt_idx);
            }
        }

        // alternative: could be elif/else_clause (Python) or if_statement/statement_block (JS)
        if let Some(alternative) = ts_node.child_by_field_name("alternative") {
            match alternative.kind() {
                "else_clause" => {
                    has_else = true;
                    // Python: else_clause has a "body" field
                    // JS: else_clause wraps a statement_block (or if_statement) directly
                    let body_node = alternative.child_by_field_name("body")
                        .or_else(|| {
                            // JS: find the first named child that is a block-like node
                            let mut c = alternative.walk();
                            alternative.named_children(&mut c).find(|child| {
                                matches!(child.kind(), "statement_block" | "if_statement")
                            })
                        });
                    if let Some(body) = body_node {
                        if body.kind() == "if_statement" {
                            // JS: else if
                            if let Some(result) = self.build_if_cfg(cpg, body, ctx, source) {
                                cpg.graph.add_edge(stmt_idx, result.entry, CpgEdge::ControlFlowFalse);
                                all_exits.extend(result.exits);
                            }
                        } else if let Some(result) = self.build_block_cfg(cpg, body, ctx, source) {
                            cpg.graph.add_edge(stmt_idx, result.entry, CpgEdge::ControlFlowFalse);
                            all_exits.extend(result.exits);
                        }
                    }
                }
                "elif_clause" => {
                    has_else = true;
                    if let Some(result) = self.build_elif_cfg(cpg, alternative, ctx, source) {
                        cpg.graph.add_edge(stmt_idx, result.entry, CpgEdge::ControlFlowFalse);
                        all_exits.extend(result.exits);
                    }
                }
                "if_statement" => {
                    // JS: else if (...) { ... } — alternative is another if_statement
                    has_else = true;
                    if let Some(result) = self.build_if_cfg(cpg, alternative, ctx, source) {
                        cpg.graph.add_edge(stmt_idx, result.entry, CpgEdge::ControlFlowFalse);
                        all_exits.extend(result.exits);
                    }
                }
                "statement_block" => {
                    // JS: else { ... } — alternative is a statement_block
                    has_else = true;
                    if let Some(result) = self.build_block_cfg(cpg, alternative, ctx, source) {
                        cpg.graph.add_edge(stmt_idx, result.entry, CpgEdge::ControlFlowFalse);
                        all_exits.extend(result.exits);
                    }
                }
                _ => {}
            }
        }

        // If no else branch, the false path falls through
        if !has_else {
            all_exits.push(stmt_idx);
        }

        Some(CfgBuildResult {
            entry: stmt_idx,
            exits: all_exits,
        })
    }

    /// Build CFG for elif clause (recursive for elif chains).
    fn build_elif_cfg(
        &self,
        cpg: &mut CpgLayer,
        ts_node: tree_sitter::Node,
        ctx: &mut BuildContext,
        source: &str,
    ) -> Option<CfgBuildResult> {
        let stmt_idx = self.create_statement_node(cpg, ts_node, ctx, source, StatementKind::If);
        let mut all_exits: Vec<NodeIndex> = Vec::new();
        let mut has_else = false;

        // consequence (true branch)
        if let Some(consequence) = ts_node.child_by_field_name("consequence") {
            if let Some(result) = self.build_block_cfg(cpg, consequence, ctx, source) {
                cpg.graph.add_edge(stmt_idx, result.entry, CpgEdge::ControlFlowTrue);
                all_exits.extend(result.exits);
            } else {
                all_exits.push(stmt_idx);
            }
        }

        // alternative
        if let Some(alternative) = ts_node.child_by_field_name("alternative") {
            match alternative.kind() {
                "else_clause" => {
                    has_else = true;
                    if let Some(body) = alternative.child_by_field_name("body") {
                        if let Some(result) = self.build_block_cfg(cpg, body, ctx, source) {
                            cpg.graph.add_edge(stmt_idx, result.entry, CpgEdge::ControlFlowFalse);
                            all_exits.extend(result.exits);
                        }
                    }
                }
                "elif_clause" => {
                    has_else = true;
                    if let Some(result) = self.build_elif_cfg(cpg, alternative, ctx, source) {
                        cpg.graph.add_edge(stmt_idx, result.entry, CpgEdge::ControlFlowFalse);
                        all_exits.extend(result.exits);
                    }
                }
                _ => {}
            }
        }

        if !has_else {
            all_exits.push(stmt_idx);
        }

        Some(CfgBuildResult {
            entry: stmt_idx,
            exits: all_exits,
        })
    }

    /// Build CFG for a for loop.
    fn build_for_cfg(
        &self,
        cpg: &mut CpgLayer,
        ts_node: tree_sitter::Node,
        ctx: &mut BuildContext,
        source: &str,
    ) -> Option<CfgBuildResult> {
        let header_idx = self.create_statement_node(cpg, ts_node, ctx, source, StatementKind::For);
        let mut all_exits: Vec<NodeIndex> = Vec::new();

        // Push loop context
        ctx.loop_stack.push(LoopContext {
            header_idx,
            break_collectors: Vec::new(),
        });

        // body (true branch — iteration continues)
        if let Some(body) = ts_node.child_by_field_name("body") {
            if let Some(result) = self.build_block_cfg(cpg, body, ctx, source) {
                cpg.graph.add_edge(header_idx, result.entry, CpgEdge::ControlFlowTrue);
                // Body exits loop back to header
                for exit in result.exits {
                    cpg.graph.add_edge(exit, header_idx, CpgEdge::ControlFlowBack);
                }
            }
        }

        // Pop loop context and collect break exits
        let loop_ctx = ctx.loop_stack.pop().unwrap();
        all_exits.extend(loop_ctx.break_collectors);

        // for/else: else clause executes if loop completes normally (no break)
        // The "false" edge from header is when iteration is exhausted
        let mut cursor = ts_node.walk();
        let else_clause = ts_node.children(&mut cursor)
            .find(|c| c.kind() == "else_clause");

        if let Some(else_node) = else_clause {
            if let Some(body) = else_node.child_by_field_name("body") {
                if let Some(result) = self.build_block_cfg(cpg, body, ctx, source) {
                    cpg.graph.add_edge(header_idx, result.entry, CpgEdge::ControlFlowFalse);
                    all_exits.extend(result.exits);
                } else {
                    all_exits.push(header_idx);
                }
            } else {
                all_exits.push(header_idx);
            }
        } else {
            // No else: false edge is the loop exit
            all_exits.push(header_idx);
        }

        Some(CfgBuildResult {
            entry: header_idx,
            exits: all_exits,
        })
    }

    /// Build CFG for a while loop.
    fn build_while_cfg(
        &self,
        cpg: &mut CpgLayer,
        ts_node: tree_sitter::Node,
        ctx: &mut BuildContext,
        source: &str,
    ) -> Option<CfgBuildResult> {
        let header_idx = self.create_statement_node(cpg, ts_node, ctx, source, StatementKind::While);
        let mut all_exits: Vec<NodeIndex> = Vec::new();

        // Push loop context
        ctx.loop_stack.push(LoopContext {
            header_idx,
            break_collectors: Vec::new(),
        });

        // body (true branch)
        if let Some(body) = ts_node.child_by_field_name("body") {
            if let Some(result) = self.build_block_cfg(cpg, body, ctx, source) {
                cpg.graph.add_edge(header_idx, result.entry, CpgEdge::ControlFlowTrue);
                // Body exits loop back to header
                for exit in result.exits {
                    cpg.graph.add_edge(exit, header_idx, CpgEdge::ControlFlowBack);
                }
            }
        }

        // Pop loop context and collect break exits
        let loop_ctx = ctx.loop_stack.pop().unwrap();
        all_exits.extend(loop_ctx.break_collectors);

        // while/else
        let mut cursor = ts_node.walk();
        let else_clause = ts_node.children(&mut cursor)
            .find(|c| c.kind() == "else_clause");

        if let Some(else_node) = else_clause {
            if let Some(body) = else_node.child_by_field_name("body") {
                if let Some(result) = self.build_block_cfg(cpg, body, ctx, source) {
                    cpg.graph.add_edge(header_idx, result.entry, CpgEdge::ControlFlowFalse);
                    all_exits.extend(result.exits);
                } else {
                    all_exits.push(header_idx);
                }
            } else {
                all_exits.push(header_idx);
            }
        } else {
            // No else: condition-false is the loop exit
            all_exits.push(header_idx);
        }

        Some(CfgBuildResult {
            entry: header_idx,
            exits: all_exits,
        })
    }

    /// Build CFG for try/except/else/finally.
    fn build_try_cfg(
        &self,
        cpg: &mut CpgLayer,
        ts_node: tree_sitter::Node,
        ctx: &mut BuildContext,
        source: &str,
    ) -> Option<CfgBuildResult> {
        let stmt_idx = self.create_statement_node(cpg, ts_node, ctx, source, StatementKind::Try);
        let mut all_exits: Vec<NodeIndex> = Vec::new();
        let mut body_exits: Vec<NodeIndex> = Vec::new();
        let mut handler_exits: Vec<NodeIndex> = Vec::new();
        let mut has_finally = false;

        // Parse children to find body, except handlers, else, finally
        let mut cursor = ts_node.walk();
        let children: Vec<_> = ts_node.children(&mut cursor).collect();

        for child in &children {
            match child.kind() {
                "block" => {
                    // Python: try body (first block child)
                    if body_exits.is_empty() {
                        if let Some(result) = self.build_block_cfg(cpg, *child, ctx, source) {
                            cpg.graph.add_edge(stmt_idx, result.entry, CpgEdge::ControlFlowNext);
                            body_exits = result.exits;
                        }
                    }
                }
                "statement_block" => {
                    // JS: try body
                    if body_exits.is_empty() {
                        if let Some(result) = self.build_block_cfg(cpg, *child, ctx, source) {
                            cpg.graph.add_edge(stmt_idx, result.entry, CpgEdge::ControlFlowNext);
                            body_exits = result.exits;
                        }
                    }
                }
                "except_clause" => {
                    // Python exception handler
                    if let Some(body) = child.child_by_field_name("body") {
                        if let Some(result) = self.build_block_cfg(cpg, body, ctx, source) {
                            cpg.graph.add_edge(stmt_idx, result.entry, CpgEdge::ControlFlowException);
                            handler_exits.extend(result.exits);
                        }
                    } else {
                        let mut inner_cursor = child.walk();
                        let inner_children: Vec<_> = child.named_children(&mut inner_cursor).collect();
                        for inner in &inner_children {
                            if inner.kind() == "block" {
                                if let Some(result) = self.build_block_cfg(cpg, *inner, ctx, source) {
                                    cpg.graph.add_edge(stmt_idx, result.entry, CpgEdge::ControlFlowException);
                                    handler_exits.extend(result.exits);
                                }
                            }
                        }
                    }
                }
                "catch_clause" => {
                    // JS: catch (e) { ... }
                    if let Some(body) = child.child_by_field_name("body") {
                        if let Some(result) = self.build_block_cfg(cpg, body, ctx, source) {
                            cpg.graph.add_edge(stmt_idx, result.entry, CpgEdge::ControlFlowException);
                            handler_exits.extend(result.exits);
                        }
                    }
                }
                "else_clause" => {
                    // Python: else clause runs if no exception
                    if let Some(body) = child.child_by_field_name("body") {
                        if let Some(result) = self.build_block_cfg(cpg, body, ctx, source) {
                            for exit in &body_exits {
                                cpg.graph.add_edge(*exit, result.entry, CpgEdge::ControlFlowNext);
                            }
                            body_exits = result.exits;
                        }
                    }
                }
                "finally_clause" => {
                    has_finally = true;
                    // JS/Python: finally block — look for block or statement_block child
                    let mut inner_cursor = child.walk();
                    let inner_children: Vec<_> = child.named_children(&mut inner_cursor).collect();
                    for inner in &inner_children {
                        if inner.kind() == "block" || inner.kind() == "statement_block" {
                            if let Some(result) = self.build_block_cfg(cpg, *inner, ctx, source) {
                                for exit in &body_exits {
                                    cpg.graph.add_edge(*exit, result.entry, CpgEdge::ControlFlowNext);
                                }
                                for exit in &handler_exits {
                                    cpg.graph.add_edge(*exit, result.entry, CpgEdge::ControlFlowNext);
                                }
                                all_exits = result.exits;
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        }

        if !has_finally {
            all_exits.extend(body_exits);
            all_exits.extend(handler_exits);
        }

        Some(CfgBuildResult {
            entry: stmt_idx,
            exits: all_exits,
        })
    }

    /// Build CFG for a with statement.
    fn build_with_cfg(
        &self,
        cpg: &mut CpgLayer,
        ts_node: tree_sitter::Node,
        ctx: &mut BuildContext,
        source: &str,
    ) -> Option<CfgBuildResult> {
        let stmt_idx = self.create_statement_node(cpg, ts_node, ctx, source, StatementKind::With);

        if let Some(body) = ts_node.child_by_field_name("body") {
            if let Some(result) = self.build_block_cfg(cpg, body, ctx, source) {
                cpg.graph.add_edge(stmt_idx, result.entry, CpgEdge::ControlFlowNext);
                Some(CfgBuildResult {
                    entry: stmt_idx,
                    exits: result.exits,
                })
            } else {
                Some(CfgBuildResult {
                    entry: stmt_idx,
                    exits: vec![stmt_idx],
                })
            }
        } else {
            Some(CfgBuildResult {
                entry: stmt_idx,
                exits: vec![stmt_idx],
            })
        }
    }

    /// Build CFG for a match statement.
    fn build_match_cfg(
        &self,
        cpg: &mut CpgLayer,
        ts_node: tree_sitter::Node,
        ctx: &mut BuildContext,
        source: &str,
    ) -> Option<CfgBuildResult> {
        let stmt_idx = self.create_statement_node(cpg, ts_node, ctx, source, StatementKind::Match);
        let mut all_exits: Vec<NodeIndex> = Vec::new();
        let mut has_cases = false;

        // Iterate children to find case_clause nodes
        let mut cursor = ts_node.walk();
        let body = ts_node.child_by_field_name("body");
        let search_node = body.unwrap_or(ts_node);
        let children: Vec<_> = search_node.named_children(&mut cursor).collect();

        for child in children {
            if child.kind() == "case_clause" {
                has_cases = true;
                // Find the body block within the case_clause
                let mut inner_cursor = child.walk();
                let case_children: Vec<_> = child.named_children(&mut inner_cursor).collect();
                for inner in case_children {
                    if inner.kind() == "block" {
                        if let Some(result) = self.build_block_cfg(cpg, inner, ctx, source) {
                            cpg.graph.add_edge(stmt_idx, result.entry, CpgEdge::ControlFlowNext);
                            all_exits.extend(result.exits);
                        }
                        break;
                    }
                }
            }
        }

        if !has_cases {
            all_exits.push(stmt_idx);
        }

        Some(CfgBuildResult {
            entry: stmt_idx,
            exits: all_exits,
        })
    }

    /// Create a CpgNode for a statement and add it to the graph.
    fn create_statement_node(
        &self,
        cpg: &mut CpgLayer,
        ts_node: tree_sitter::Node,
        ctx: &mut BuildContext,
        source: &str,
        kind: StatementKind,
    ) -> NodeIndex {
        let name = get_statement_label(ts_node, source, &kind);

        let node = CpgNode {
            file_path: ctx.file_path.clone(),
            name,
            kind: CpgNodeKind::Statement,
            start_byte: ts_node.start_byte(),
            end_byte: ts_node.end_byte(),
            start_line: ts_node.start_position().row + 1,
            end_line: ts_node.end_position().row + 1,
            parameters: Vec::new(),
            return_type: None,
            docstring: None,
            bases: Vec::new(),
            parent_class: None,
            statement_kind: Some(kind),
            function_idx: Some(ctx.func_idx),
        };

        let idx = cpg.graph.add_node(node);
        cpg.function_to_stmts.entry(ctx.func_idx).or_default().push(idx);

        // Add AstChild edge from function to statement
        cpg.graph.add_edge(ctx.func_idx, idx, CpgEdge::AstChild);

        // Add to file_to_nodes
        if let Some(node_list) = cpg.file_to_nodes.get_mut(&ctx.file_path) {
            node_list.push(idx);
        }

        idx
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Classify a tree-sitter node into a StatementKind.
fn classify_statement(ts_node: tree_sitter::Node) -> StatementKind {
    match ts_node.kind() {
        // Python + JS/TS shared
        "if_statement" => StatementKind::If,
        "while_statement" => StatementKind::While,
        "try_statement" => StatementKind::Try,
        "return_statement" => StatementKind::Return,
        "break_statement" => StatementKind::Break,
        "continue_statement" => StatementKind::Continue,
        "expression_statement" => StatementKind::ExpressionStatement,

        // Python-specific
        "for_statement" => StatementKind::For,
        "with_statement" => StatementKind::With,
        "match_statement" => StatementKind::Match,
        "raise_statement" => StatementKind::Raise,
        "assignment" => StatementKind::Assignment,
        "augmented_assignment" => StatementKind::Assignment,
        "assert_statement" => StatementKind::Assert,
        "pass_statement" => StatementKind::Pass,
        "import_statement" | "import_from_statement" => StatementKind::Import,
        "delete_statement" => StatementKind::Delete,
        "elif_clause" => StatementKind::If,

        // JS/TS-specific
        "for_in_statement" => StatementKind::For,
        "do_statement" => StatementKind::While,
        "switch_statement" => StatementKind::Match,
        "throw_statement" => StatementKind::Raise,
        "variable_declaration" | "lexical_declaration" => StatementKind::Assignment,
        "export_statement" => StatementKind::Import,

        other => StatementKind::Other(other.to_string()),
    }
}

/// Generate a short human-readable label for a statement node.
fn get_statement_label(ts_node: tree_sitter::Node, source: &str, kind: &StatementKind) -> String {
    // Take first line of the statement, truncated
    let text = ts_node.utf8_text(source.as_bytes()).unwrap_or("");
    let first_line = text.lines().next().unwrap_or("");
    let truncated = if first_line.len() > 60 {
        // Find a char boundary at or before byte 60
        let mut end = 60;
        while end > 0 && !first_line.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &first_line[..end])
    } else {
        first_line.to_string()
    };
    format!("{:?}: {}", kind, truncated)
}

/// Find a tree-sitter node that exactly matches the given byte range.
pub(crate) fn find_ts_node_at_bytes<'a>(
    root: tree_sitter::Node<'a>,
    start_byte: usize,
    end_byte: usize,
) -> Option<tree_sitter::Node<'a>> {
    // Walk tree looking for exact match
    let mut cursor = root.walk();

    // Try direct children first
    if root.start_byte() == start_byte && root.end_byte() == end_byte {
        return Some(root);
    }

    if cursor.goto_first_child() {
        loop {
            let node = cursor.node();
            if node.start_byte() == start_byte && node.end_byte() == end_byte {
                return Some(node);
            }
            // Recurse into children that contain our range
            if node.start_byte() <= start_byte && node.end_byte() >= end_byte {
                if let Some(found) = find_ts_node_at_bytes(node, start_byte, end_byte) {
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
