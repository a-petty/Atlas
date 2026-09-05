//! API-oriented source views. Retained ranges always refer to original UTF-8
//! bytes; generated placeholders and layout never claim source coverage.
use crate::parser::SupportedLanguage;
use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Tree};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainedSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkeletonChunk {
    pub text: String,
    pub spans: Vec<RetainedSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkeletonResult {
    pub text: String,
    pub chunks: Vec<SkeletonChunk>,
}

#[derive(Clone)]
enum Part {
    Source(usize, usize),
    Generated(String),
}

type Parts = Vec<Part>;
const CONTAINER_CHUNK_BYTES: usize = 4096;
const LARGE_INITIALIZER_BYTES: usize = 1024;

fn keep(parts: &mut Parts, start: usize, end: usize) {
    if start >= end {
        return;
    }
    if let Some(Part::Source(_, previous_end)) = parts.last_mut() {
        if *previous_end == start {
            *previous_end = end;
            return;
        }
    }
    parts.push(Part::Source(start, end));
}

fn generated(parts: &mut Parts, text: &str) {
    if !text.is_empty() {
        parts.push(Part::Generated(text.to_owned()));
    }
}

struct View<'a> {
    source: &'a str,
    language: SupportedLanguage,
    line_starts: Vec<usize>,
}

impl<'a> View<'a> {
    fn new(source: &'a str, language: SupportedLanguage) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(source.match_indices('\n').map(|(i, _)| i + 1));
        Self {
            source,
            language,
            line_starts,
        }
    }

    fn line(&self, byte: usize) -> usize {
        self.line_starts.partition_point(|start| *start <= byte)
    }

    fn indent_start(&self, byte: usize) -> usize {
        let start = self.line_starts[self.line(byte) - 1];
        if self.source[start..byte]
            .chars()
            .all(|c| c == ' ' || c == '\t')
        {
            start
        } else {
            byte
        }
    }

    fn finish(&self, parts: Parts) -> SkeletonChunk {
        let mut text = String::new();
        let mut spans = Vec::new();
        for part in parts {
            match part {
                Part::Source(start, end) => {
                    text.push_str(&self.source[start..end]);
                    spans.push(RetainedSpan {
                        start_byte: start,
                        end_byte: end,
                        start_line: self.line(start),
                        end_line: self.line(end - 1),
                    });
                }
                Part::Generated(value) => text.push_str(&value),
            }
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        SkeletonChunk { text, spans }
    }

    fn is_python(&self) -> bool {
        self.language == SupportedLanguage::Python
    }

    fn is_docstring(&self, node: Node) -> bool {
        node.kind() == "expression_statement"
            && node.named_child(0).map_or(false, |child| {
                matches!(child.kind(), "string" | "concatenated_string")
            })
    }

    fn is_annotation(&self, node: Node) -> bool {
        matches!(
            node.kind(),
            "comment"
                | "line_comment"
                | "block_comment"
                | "attribute_item"
                | "inner_attribute_item"
                | "hash_bang_line"
        )
    }

    fn is_callable(&self, node: Node) -> bool {
        matches!(
            node.kind(),
            "function_definition"
                | "async_function_definition"
                | "function_item"
                | "function_declaration"
                | "generator_function_declaration"
                | "method_declaration"
                | "method_definition"
                | "function"
                | "function_expression"
                | "generator_function"
                | "arrow_function"
                | "func_literal"
                | "closure_expression"
                | "lambda"
                | "class_static_block"
        )
    }

    fn is_python_declaration(&self, node: Node) -> bool {
        matches!(
            node.kind(),
            "function_definition"
                | "class_definition"
                | "decorated_definition"
                | "async_function_definition"
                | "import_statement"
                | "import_from_statement"
                | "type_alias_statement"
        ) || (node.kind() == "expression_statement"
            && node.named_child(0).map_or(false, |n| {
                matches!(n.kind(), "assignment" | "augmented_assignment")
            }))
    }

    fn is_api(&self, node: Node) -> bool {
        if self.is_annotation(node) {
            return true;
        }
        if self.is_python() {
            return self.is_python_declaration(node)
                || self.is_docstring(node)
                || self.python_guard_has_api(node);
        }
        match self.language {
            SupportedLanguage::Rust => matches!(
                node.kind(),
                "use_declaration"
                    | "extern_crate_declaration"
                    | "function_item"
                    | "function_signature_item"
                    | "struct_item"
                    | "enum_item"
                    | "trait_item"
                    | "impl_item"
                    | "mod_item"
                    | "const_item"
                    | "static_item"
                    | "type_item"
                    | "associated_type"
                    | "foreign_mod_item"
                    | "macro_definition"
            ),
            SupportedLanguage::Go => matches!(
                node.kind(),
                "package_clause"
                    | "import_declaration"
                    | "function_declaration"
                    | "method_declaration"
                    | "type_declaration"
                    | "var_declaration"
                    | "const_declaration"
            ),
            _ => {
                matches!(
                    node.kind(),
                    "import_statement"
                        | "export_statement"
                        | "lexical_declaration"
                        | "variable_declaration"
                        | "function_declaration"
                        | "generator_function_declaration"
                        | "class_declaration"
                        | "abstract_class_declaration"
                        | "interface_declaration"
                        | "type_alias_declaration"
                        | "enum_declaration"
                        | "internal_module"
                        | "module"
                        | "ambient_declaration"
                        | "function_signature"
                        | "import_alias"
                ) || self.is_commonjs_export(node)
            }
        }
    }

    fn is_commonjs_export(&self, node: Node) -> bool {
        if node.kind() != "expression_statement" {
            return false;
        }
        let Some(assignment) = node.named_child(0) else {
            return false;
        };
        if assignment.kind() != "assignment_expression" {
            return false;
        }
        assignment
            .child_by_field_name("left")
            .map_or(false, |left| {
                let text = &self.source[left.byte_range()];
                text == "module.exports"
                    || text.starts_with("module.exports.")
                    || text.starts_with("exports.")
                    || text.starts_with("exports[")
                    || text.starts_with("module.exports[")
            })
    }

    fn python_guard_has_api(&self, node: Node) -> bool {
        if !matches!(
            node.kind(),
            "if_statement"
                | "elif_clause"
                | "else_clause"
                | "try_statement"
                | "except_clause"
                | "finally_clause"
                | "with_statement"
        ) {
            return false;
        }
        let mut cursor = node.walk();
        node.named_children(&mut cursor).any(|child| {
            if child.kind() == "block" {
                let mut inner = child.walk();
                child
                    .named_children(&mut inner)
                    .any(|n| self.is_python_declaration(n) || self.python_guard_has_api(n))
            } else {
                self.python_guard_has_api(child)
            }
        })
    }

    fn body_container<'n>(&self, node: Node<'n>) -> Option<Node<'n>> {
        match node.kind() {
            "class_definition"
            | "class_declaration"
            | "abstract_class_declaration"
            | "class"
            | "impl_item"
            | "trait_item"
            | "mod_item"
            | "foreign_mod_item"
            | "internal_module"
            | "module" => node.child_by_field_name("body"),
            _ => None,
        }
    }

    fn wrapped_container<'n>(&self, node: Node<'n>) -> Option<(Node<'n>, Node<'n>)> {
        if let Some(body) = self.body_container(node) {
            return Some((node, body));
        }
        if matches!(
            node.kind(),
            "decorated_definition" | "export_statement" | "ambient_declaration"
        ) {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(found) = self.wrapped_container(child) {
                    return Some(found);
                }
            }
        }
        None
    }

    fn render(&self, node: Node, parts: &mut Parts) {
        if self.is_callable(node) {
            if let Some(body) = node.child_by_field_name("body") {
                keep(parts, node.start_byte(), body.start_byte());
                self.callable_body(body, parts);
                keep(parts, body.end_byte(), node.end_byte());
                return;
            }
        }
        if self.is_python() && node.kind() == "class_definition" {
            if let Some(body) = node.child_by_field_name("body") {
                let header_end = self.indent_start(body.start_byte());
                keep(parts, node.start_byte(), header_end);
                self.python_block(body, parts, false);
                return;
            }
        }
        if self.is_python() && self.python_guard_has_api(node) {
            self.python_guard(node, parts);
            return;
        }
        if node.kind() == "macro_definition" {
            // Preserve each macro matcher (the public invocation shape), but
            // omit expansion code, which may be arbitrarily large.
            self.render_children(node, parts, true);
            return;
        }
        if let Some(value) = self.large_initializer(node) {
            keep(parts, node.start_byte(), value.start_byte());
            generated(parts, "...");
            keep(parts, value.end_byte(), node.end_byte());
            return;
        }
        self.render_children(node, parts, false);
    }

    fn large_initializer<'n>(&self, node: Node<'n>) -> Option<Node<'n>> {
        let field = match node.kind() {
            "assignment" => "right",
            "variable_declarator"
            | "public_field_definition"
            | "field_definition"
            | "const_item"
            | "static_item"
            | "const_spec"
            | "var_spec" => "value",
            _ => return None,
        };
        node.child_by_field_name(field).filter(|value| {
            value.end_byte() - value.start_byte() > LARGE_INITIALIZER_BYTES
                && !self.is_callable(*value)
        })
    }

    fn render_children(&self, node: Node, parts: &mut Parts, macro_definition: bool) {
        let mut cursor = node.walk();
        let mut end = node.start_byte();
        for child in node.named_children(&mut cursor) {
            keep(parts, end, child.start_byte());
            if macro_definition && child.kind() == "macro_rule" {
                if let Some(right) = child.child_by_field_name("right") {
                    keep(parts, child.start_byte(), right.start_byte());
                    generated(parts, "{ /* ... */ }");
                    keep(parts, right.end_byte(), child.end_byte());
                } else {
                    self.render(child, parts);
                }
            } else {
                self.render(child, parts);
            }
            end = child.end_byte();
        }
        keep(parts, end, node.end_byte());
    }

    fn callable_body(&self, body: Node, parts: &mut Parts) {
        if self.is_python() && body.kind() == "block" {
            self.python_block(body, parts, true);
        } else if matches!(body.kind(), "block" | "statement_block") {
            keep(parts, body.start_byte(), body.start_byte() + 1);
            // Leading documentation and nested declarations are useful APIs;
            // executable statements and their incidental comments are not.
            let mut cursor = body.walk();
            let mut leading = true;
            for child in body.named_children(&mut cursor) {
                let nested_api = matches!(
                    child.kind(),
                    "function_item"
                        | "function_declaration"
                        | "generator_function_declaration"
                        | "class_declaration"
                        | "struct_item"
                        | "enum_item"
                        | "trait_item"
                        | "type_item"
                );
                if nested_api || (leading && self.is_annotation(child)) {
                    generated(parts, "\n");
                    keep(
                        parts,
                        self.indent_start(child.start_byte()),
                        child.start_byte(),
                    );
                    self.render(child, parts);
                } else {
                    leading = false;
                }
            }
            generated(parts, " /* ... */ ");
            // Braces have source attribution; the replacement body has none.
            if self
                .source
                .as_bytes()
                .get(body.end_byte().saturating_sub(1))
                == Some(&b'}')
            {
                keep(parts, body.end_byte() - 1, body.end_byte());
            }
        } else {
            // Expression-bodied arrows, lambdas and Rust closures.
            generated(parts, "...");
        }
    }

    fn python_block(&self, body: Node, parts: &mut Parts, function_body: bool) {
        let mut cursor = body.walk();
        let mut kept = false;
        let mut seen_statement = false;
        for child in body.named_children(&mut cursor) {
            let nested_definition = matches!(
                child.kind(),
                "function_definition"
                    | "class_definition"
                    | "decorated_definition"
                    | "async_function_definition"
            );
            let retain = if function_body {
                (!seen_statement && (self.is_docstring(child) || self.is_annotation(child)))
                    || nested_definition
            } else {
                self.is_api(child)
            };
            if !self.is_annotation(child) {
                seen_statement = true;
            }
            if retain {
                // A callable header already contains indentation before its
                // first body token. Other members need their original indent.
                if kept || !function_body {
                    if kept {
                        generated(parts, "\n");
                    }
                    keep(
                        parts,
                        self.indent_start(child.start_byte()),
                        child.start_byte(),
                    );
                }
                self.render(child, parts);
                kept = true;
            }
        }
        if function_body || !kept {
            if kept {
                generated(parts, "\n");
                keep(
                    parts,
                    self.indent_start(body.start_byte()),
                    body.start_byte(),
                );
            }
            generated(parts, "...");
        }
    }

    fn python_guard(&self, node: Node, parts: &mut Parts) {
        let mut cursor = node.walk();
        let mut end = node.start_byte();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "block" {
                keep(parts, end, self.indent_start(child.start_byte()));
                self.python_block(child, parts, false);
                end = child.end_byte();
            } else if self.python_guard_has_api(child) {
                keep(parts, end, child.start_byte());
                self.python_guard(child, parts);
                end = child.end_byte();
            } else if matches!(
                child.kind(),
                "else_clause" | "elif_clause" | "except_clause" | "finally_clause"
            ) {
                // A branch with only executable code is omitted completely.
                end = child.end_byte();
            }
        }
        keep(parts, end, node.end_byte());
    }

    fn chunks(&self, node: Node, prefix: &[Part], suffix: &[Part]) -> Vec<SkeletonChunk> {
        let mut complete = prefix.to_vec();
        self.render(node, &mut complete);
        complete.extend_from_slice(suffix);
        let full = self.finish(complete);
        let Some((_container, body)) = self.wrapped_container(node) else {
            return vec![full];
        };
        if full.text.len() <= CONTAINER_CHUNK_BYTES {
            return vec![full];
        }
        // Split a large class/impl/module at member boundaries, repeating its
        // actual header so every chunk identifies its enclosing declaration.
        let python_body = self.is_python() && body.kind() == "block";
        let header_end = if python_body {
            self.indent_start(body.start_byte())
        } else {
            body.start_byte() + 1
        };
        let mut head = prefix.to_vec();
        keep(&mut head, node.start_byte(), header_end);
        if !python_body {
            generated(&mut head, "\n");
        }
        let mut tail = Vec::new();
        if !python_body {
            generated(&mut tail, "\n");
            keep(&mut tail, body.end_byte() - 1, node.end_byte());
        }
        tail.extend_from_slice(suffix);
        let mut chunks = Vec::new();
        let mut pending = head.clone();
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            if python_body && !self.is_api(child) {
                continue;
            }
            keep(
                &mut pending,
                self.indent_start(child.start_byte()),
                child.start_byte(),
            );
            if self.is_annotation(child) {
                self.render(child, &mut pending);
                generated(&mut pending, "\n");
                continue;
            }
            chunks.extend(self.chunks(child, &pending, &tail));
            pending = head.clone();
        }
        if chunks.is_empty() {
            vec![full]
        } else {
            chunks
        }
    }
}

pub fn uncompressed(source: &str) -> SkeletonResult {
    if source.is_empty() {
        return SkeletonResult {
            text: String::new(),
            chunks: Vec::new(),
        };
    }
    let view = View::new(source, SupportedLanguage::Unknown);
    let mut chunk = view.finish(vec![Part::Source(0, source.len())]);
    // The historical unsupported-language fallback is byte-for-byte unchanged.
    chunk.text = source.to_owned();
    SkeletonResult {
        text: source.to_owned(),
        chunks: vec![chunk],
    }
}

pub fn create_skeleton_details(
    source: &str,
    tree: &Tree,
    language: SupportedLanguage,
) -> SkeletonResult {
    if language == SupportedLanguage::Unknown {
        return uncompressed(source);
    }
    let view = View::new(source, language);
    let mut chunks = Vec::new();
    let mut pending = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for node in root.named_children(&mut cursor) {
        if view.is_annotation(node) {
            view.render(node, &mut pending);
            generated(&mut pending, "\n");
        } else if view.is_api(node) {
            chunks.extend(view.chunks(node, &pending, &[]));
            pending.clear();
        } else if !pending.is_empty() {
            chunks.push(view.finish(std::mem::take(&mut pending)));
        }
    }
    if !pending.is_empty() {
        chunks.push(view.finish(pending));
    }
    let text = chunks
        .iter()
        .map(|chunk| chunk.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    SkeletonResult { text, chunks }
}
