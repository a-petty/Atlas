//! AST-only conservative call index: no CFG nodes, no live-disk reads.
use crate::import_resolver::ImportBinding;
use crate::parser::SupportedLanguage;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CallableId {
    pub file_path: PathBuf,
    pub qualified_name: String,
    pub start_byte: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callable {
    pub id: CallableId,
    pub file_path: PathBuf,
    pub name: String,
    pub qualified_name: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub parent_class: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
enum Value {
    Function(CallableId),
    Class(PathBuf, String),
    Instance(PathBuf, String),
    Module(PathBuf),
    ModulePrefix(PathBuf, Vec<String>),
}
#[derive(Debug, Clone)]
enum Binding {
    Value(Value),
    Import(String),
    Unknown,
    Expression {
        parts: Vec<String>,
        instance: bool,
        position: usize,
    },
}
#[derive(Debug, Clone, Default)]
struct Scope {
    parent: Option<usize>,
    qualifier: String,
    class: bool,
    bindings: BTreeMap<String, Vec<Binding>>,
}
#[derive(Debug, Clone)]
struct Site {
    caller: CallableId,
    scope: usize,
    parts: Vec<String>,
    start: usize,
}
#[derive(Debug, Clone, Default)]
struct FileIndex {
    scopes: Vec<Scope>,
    declarations: BTreeMap<CallableId, Callable>,
    class_scopes: BTreeMap<String, usize>,
    sites: Vec<Site>,
    imports: Vec<ImportBinding>,
    potential_imports: BTreeSet<String>,
    references: Vec<(usize, String)>,
    javascript: bool,
    exports: BTreeMap<String, Vec<Binding>>,
    static_methods: BTreeSet<CallableId>,
    commonjs_roots: BTreeMap<String, String>,
    module_prefixes: BTreeMap<String, Vec<String>>,
}
#[derive(Debug, Clone, Default)]
pub struct CallIndex {
    files: BTreeMap<PathBuf, FileIndex>,
    outgoing: BTreeMap<CallableId, BTreeSet<CallableId>>,
    incoming: BTreeMap<CallableId, BTreeSet<CallableId>>,
    resolved_sites: BTreeMap<(PathBuf, usize), CallableId>,
}
impl CallIndex {
    pub fn new() -> Self {
        Self::default()
    }
    /// Replace AST data; publish resolution after all source/binding updates.
    pub fn update_file(
        &mut self,
        path: &Path,
        tree: &Tree,
        source: &str,
        language: SupportedLanguage,
        bindings: Vec<ImportBinding>,
    ) {
        let mut file = FileIndex {
            scopes: vec![Scope::default()],
            imports: bindings,
            javascript: matches!(
                language,
                SupportedLanguage::JavaScript
                    | SupportedLanguage::JavaScriptJsx
                    | SupportedLanguage::TypeScript
                    | SupportedLanguage::TypeScriptTsx
            ),
            ..FileIndex::default()
        };
        if matches!(
            language,
            SupportedLanguage::Python
                | SupportedLanguage::JavaScript
                | SupportedLanguage::JavaScriptJsx
                | SupportedLanguage::TypeScript
                | SupportedLanguage::TypeScriptTsx
        ) {
            walk(tree.root_node(), source, path, &mut file, 0, None, false);
        }
        file.sites.sort_by_key(|site| site.start);
        file.references.sort();
        file.references.dedup();
        let shadowed_exports: Vec<_> = file
            .commonjs_roots
            .iter()
            .filter(|(_, root)| file.scopes[0].bindings.contains_key(root.as_str()))
            .map(|(name, _)| name.clone())
            .collect();
        for name in shadowed_exports {
            file.exports.remove(&name);
        }
        self.files.insert(path.to_path_buf(), file);
    }
    pub fn remove_file(&mut self, path: &Path) {
        self.files.remove(path);
    }
    pub fn set_import_bindings(&mut self, path: &Path, bindings: Vec<ImportBinding>) {
        if let Some(f) = self.files.get_mut(path) {
            f.imports = bindings;
        }
    }
    pub fn has_file(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }
    pub fn file_count(&self) -> usize {
        self.files.len()
    }
    pub fn potential_imports(&self, path: &Path) -> Vec<String> {
        self.files
            .get(path)
            .map(|f| f.potential_imports.iter().cloned().collect())
            .unwrap_or_default()
    }
    /// Direct import provenance, with lexical shadowing respected. This is
    /// independent of callable resolution and includes constants and types.
    pub fn used_import_paths(&self, path: &Path) -> BTreeSet<PathBuf> {
        let Some(file) = self.files.get(path) else {
            return BTreeSet::new();
        };
        let mut used = BTreeSet::new();
        for (initial, name) in &file.references {
            let mut scope = Some(*initial);
            while let Some(idx) = scope {
                let s = &file.scopes[idx];
                if let Some(bindings) = s.bindings.get(name) {
                    if bindings
                        .iter()
                        .all(|b| matches!(b, Binding::Import(local) if local == name))
                    {
                        let imports: Vec<_> = file
                            .imports
                            .iter()
                            .filter(|b| b.local_name == *name)
                            .collect();
                        if imports.iter().any(|b| {
                            b.imported_symbol
                                .as_deref()
                                .map(|s| s.starts_with("#require:"))
                                .unwrap_or(false)
                        }) && self.bound_name(path, idx, "require")
                        {
                            break;
                        }
                        if imports.iter().any(|b| {
                            b.imported_symbol
                                .as_ref()
                                .map(|symbol| {
                                    !self.export_exists(
                                        &b.resolved_path,
                                        symbol,
                                        &mut BTreeSet::new(),
                                    )
                                })
                                .unwrap_or(!self.files.contains_key(&b.resolved_path))
                        }) {
                            break;
                        }
                        let paths: BTreeSet<_> =
                            imports.iter().map(|b| b.resolved_path.clone()).collect();
                        if paths.len() == 1 || name == "#export:*" {
                            used.extend(paths);
                        }
                    }
                    break;
                }
                scope = s.parent;
                while let Some(parent) = scope {
                    if !file.scopes[parent].class {
                        break;
                    }
                    scope = file.scopes[parent].parent;
                }
            }
        }
        used
    }
    fn export_exists(
        &self,
        path: &Path,
        name: &str,
        visiting: &mut BTreeSet<(PathBuf, String)>,
    ) -> bool {
        let name = name
            .strip_prefix("#type:")
            .or_else(|| name.strip_prefix("#require:"))
            .unwrap_or(name);
        let Some(file) = self.files.get(path) else {
            return false;
        };
        if name.is_empty() || name == "*" {
            return true;
        }
        let key = (path.to_path_buf(), name.to_owned());
        if visiting.len() > 128 || !visiting.insert(key.clone()) {
            return false;
        }
        let bindings = if file.javascript {
            file.exports.get(name)
        } else {
            file.scopes[0].bindings.get(name)
        };
        let result = if let Some(bindings) = bindings {
            bindings.iter().any(|b| match b {
                Binding::Import(local) => {
                    let imports: Vec<_> = file
                        .imports
                        .iter()
                        .filter(|b| b.local_name == *local)
                        .collect();
                    !imports.is_empty()
                        && imports.iter().all(|b| {
                            b.imported_symbol
                                .as_ref()
                                .map(|symbol| {
                                    self.export_exists(&b.resolved_path, symbol, visiting)
                                })
                                .unwrap_or(self.files.contains_key(&b.resolved_path))
                        })
                }
                Binding::Expression { parts, .. } if file.javascript => parts
                    .first()
                    .map(|name| file.scopes[0].bindings.contains_key(name))
                    .unwrap_or(false),
                _ => true,
            })
        } else if file.javascript && name != "default" {
            file.imports
                .iter()
                .filter(|b| b.local_name == "#export:*")
                .any(|b| self.export_exists(&b.resolved_path, name, visiting))
        } else {
            false
        };
        visiting.remove(&key);
        result
    }
    pub fn functions(&self, path: &Path) -> Vec<Callable> {
        self.files
            .get(path)
            .map(|f| f.declarations.values().cloned().collect())
            .unwrap_or_default()
    }
    /// Return every candidate; a simple-name query must not select arbitrarily.
    pub fn find_functions(&self, path: &Path, name: &str) -> Vec<Callable> {
        let mut candidates = BTreeMap::new();
        for function in self
            .functions(path)
            .into_iter()
            .filter(|f| f.name == name || f.qualified_name == name)
        {
            candidates.insert(function.qualified_name.clone(), function);
        }
        candidates.into_values().collect()
    }
    pub fn callable(&self, id: &CallableId) -> Option<&Callable> {
        self.files.get(&id.file_path)?.declarations.get(id)
    }
    pub fn callees(&self, id: &CallableId) -> Vec<Callable> {
        self.outgoing
            .get(id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.callable(id).cloned())
            .collect()
    }
    pub fn callers(&self, id: &CallableId) -> Vec<Callable> {
        self.incoming
            .get(id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.callable(id).cloned())
            .collect()
    }
    pub fn resolved_target(&self, path: &Path, start: usize) -> Option<&CallableId> {
        self.resolved_sites.get(&(path.to_path_buf(), start))
    }
    pub fn call_owner(&self, path: &Path, start: usize) -> Option<&CallableId> {
        let sites = &self.files.get(path)?.sites;
        let i = sites.binary_search_by_key(&start, |s| s.start).ok()?;
        Some(&sites[i].caller)
    }
    pub fn resolve_all(&mut self) {
        self.outgoing.clear();
        self.incoming.clear();
        self.resolved_sites.clear();
        let paths: Vec<_> = self.files.keys().cloned().collect();
        self.resolve_paths(&paths);
    }
    /// Potential imports remain indexed when their modules do not exist yet.
    /// The caller must refresh resolver bindings before resolution.
    pub fn affected_files(&self, changed: &[PathBuf]) -> Vec<PathBuf> {
        let mut reverse: HashMap<PathBuf, BTreeSet<PathBuf>> = HashMap::new();
        let mut tokens: HashMap<String, BTreeSet<PathBuf>> = HashMap::new();
        for (path, f) in &self.files {
            for b in &f.imports {
                reverse
                    .entry(b.resolved_path.clone())
                    .or_default()
                    .insert(path.clone());
            }
            for token in &f.potential_imports {
                tokens
                    .entry(token.clone())
                    .or_default()
                    .insert(path.clone());
            }
        }
        // Old edges remain relevant when exports are renamed or deleted.
        for (target, callers) in &self.incoming {
            for caller in callers {
                reverse
                    .entry(target.file_path.clone())
                    .or_default()
                    .insert(caller.file_path.clone());
            }
        }
        let mut affected: BTreeSet<_> = changed.iter().cloned().collect();
        let mut queue = changed.to_vec();
        while let Some(path) = queue.pop() {
            let mut next = reverse.get(&path).cloned().unwrap_or_default();
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let token = if matches!(stem, "__init__" | "index") {
                path.parent()
                    .and_then(|p| p.file_name())
                    .and_then(|s| s.to_str())
                    .unwrap_or(stem)
            } else {
                stem
            };
            if let Some(paths) = tokens.get(token) {
                next.extend(paths.iter().cloned());
            }
            for other in next {
                if affected.insert(other.clone()) {
                    queue.push(other);
                }
            }
        }
        affected.into_iter().collect()
    }
    pub fn resolve_changed(&mut self, changed: &[PathBuf]) {
        self.resolve_changed_files(changed);
    }
    /// Resolve and return the complete affected file set. File-record updates
    /// preserve old call edges until this method computes invalidation, so the
    /// returned set includes callers of removed targets as well as current
    /// imports and unresolved potential bindings. Reuse it for semantic edges.
    pub fn resolve_changed_files(&mut self, changed: &[PathBuf]) -> Vec<PathBuf> {
        let paths = self.affected_files(changed);
        let set: BTreeSet<_> = paths.iter().cloned().collect();
        self.outgoing.retain(|id, _| !set.contains(&id.file_path));
        self.resolved_sites.retain(|(p, _), _| !set.contains(p));
        self.incoming.clear();
        for (caller, targets) in &self.outgoing {
            for target in targets {
                self.incoming
                    .entry(target.clone())
                    .or_default()
                    .insert(caller.clone());
            }
        }
        self.resolve_paths(&paths);
        paths
    }
    fn resolve_paths(&mut self, paths: &[PathBuf]) {
        for path in paths {
            let Some(file) = self.files.get(path) else {
                continue;
            };
            let resolved: Vec<_> = file
                .sites
                .iter()
                .filter_map(|site| {
                    match self.expression(
                        path,
                        site.scope,
                        &site.parts,
                        site.start,
                        &mut BTreeSet::new(),
                    ) {
                        Some(Value::Function(target)) => Some((site.clone(), target)),
                        _ => None,
                    }
                })
                .collect();
            for (site, target) in resolved {
                self.outgoing
                    .entry(site.caller.clone())
                    .or_default()
                    .insert(target.clone());
                self.incoming
                    .entry(target.clone())
                    .or_default()
                    .insert(site.caller);
                self.resolved_sites
                    .insert((path.clone(), site.start), target);
            }
        }
    }
    fn lookup(
        &self,
        path: &Path,
        scope: usize,
        name: &str,
        position: usize,
        visiting: &mut BTreeSet<(PathBuf, usize, String)>,
    ) -> Option<Value> {
        let key = (path.to_path_buf(), scope, name.to_owned());
        if visiting.len() > 128 || !visiting.insert(key.clone()) {
            return None;
        }
        let value = self.lookup_inner(path, scope, name, position, visiting);
        visiting.remove(&key);
        value
    }
    fn lookup_inner(
        &self,
        path: &Path,
        scope: usize,
        name: &str,
        position: usize,
        visiting: &mut BTreeSet<(PathBuf, usize, String)>,
    ) -> Option<Value> {
        let file = self.files.get(path)?;
        let s = file.scopes.get(scope)?;
        if let Some(bindings) = s.bindings.get(name) {
            // Repeated definitions bind to the last declaration of this exact
            // qualified callable; never use body size to select across classes.
            if bindings
                .iter()
                .all(|b| matches!(b, Binding::Value(Value::Function(_))))
            {
                if let Some(Binding::Value(Value::Function(last))) = bindings.last() {
                    if bindings.iter().all(|b|matches!(b,Binding::Value(Value::Function(id))if id.qualified_name==last.qualified_name&&id.file_path==last.file_path)) {
                        return Some(Value::Function(last.clone()));
                    }
                }
            }
            if bindings.len() != 1 {
                return None;
            }
            return match &bindings[0] {
                Binding::Value(v) => Some(v.clone()),
                Binding::Unknown => None,
                Binding::Expression {
                    parts,
                    instance,
                    position: assigned,
                } => {
                    if *assigned >= position {
                        return None;
                    }
                    let value = self.expression(path, scope, parts, *assigned, visiting)?;
                    if *instance {
                        match value {
                            Value::Class(p, n) => Some(Value::Instance(p, n)),
                            _ => None,
                        }
                    } else {
                        Some(value)
                    }
                }
                Binding::Import(local) => self.import_value(path, scope, local, visiting),
            };
        }
        // Method bodies do not close over a Python class namespace.
        let mut parent = s.parent;
        while let Some(idx) = parent {
            if !file.scopes[idx].class {
                return self.lookup(path, idx, name, position, visiting);
            }
            parent = file.scopes[idx].parent;
        }
        None
    }
    fn bound_name(&self, path: &Path, scope: usize, name: &str) -> bool {
        let Some(file) = self.files.get(path) else {
            return false;
        };
        let mut scope = Some(scope);
        while let Some(idx) = scope {
            if file.scopes[idx].bindings.contains_key(name) {
                return true;
            }
            scope = file.scopes[idx].parent;
        }
        false
    }
    fn import_value(
        &self,
        path: &Path,
        scope: usize,
        local: &str,
        visiting: &mut BTreeSet<(PathBuf, usize, String)>,
    ) -> Option<Value> {
        let file = self.files.get(path)?;
        let mut values = Vec::new();
        for binding in file.imports.iter().filter(|b| b.local_name == local) {
            let value = match binding.imported_symbol.as_deref() {
                Some(symbol) if symbol.starts_with("#type:") => return None,
                Some(symbol) if symbol.starts_with("#require:") => {
                    if self.bound_name(path, scope, "require") {
                        return None;
                    }
                    let symbol = &symbol[9..];
                    if symbol.is_empty() {
                        let target = self.files.get(&binding.resolved_path)?;
                        if target.exports.contains_key("default") {
                            self.export_value(&binding.resolved_path, "default", visiting)
                        } else {
                            Some(Value::Module(binding.resolved_path.clone()))
                        }
                    } else {
                        self.export_value(&binding.resolved_path, symbol, visiting)
                    }
                }
                Some(symbol) => self.export_value(&binding.resolved_path, symbol, visiting),
                None => Some(if let Some(prefix) = file.module_prefixes.get(local) {
                    Value::ModulePrefix(binding.resolved_path.clone(), prefix.clone())
                } else {
                    Value::Module(binding.resolved_path.clone())
                }),
            }?;
            if !values.contains(&value) {
                values.push(value);
            }
        }
        if values.len() == 1 {
            values.pop()
        } else {
            None
        }
    }
    fn export_value(
        &self,
        path: &Path,
        name: &str,
        visiting: &mut BTreeSet<(PathBuf, usize, String)>,
    ) -> Option<Value> {
        let file = self.files.get(path)?;
        if !file.javascript {
            return self.lookup(path, 0, name, usize::MAX, visiting);
        }
        let key = (path.to_path_buf(), usize::MAX, name.to_owned());
        if visiting.len() > 128 || !visiting.insert(key.clone()) {
            return None;
        }
        let result = (|| {
            if let Some(bindings) = file.exports.get(name) {
                if bindings.len() != 1 {
                    return None;
                }
                return match &bindings[0] {
                    Binding::Value(v) => Some(v.clone()),
                    Binding::Expression { parts, .. } => {
                        self.expression(path, 0, parts, usize::MAX, visiting)
                    }
                    Binding::Import(local) => self.import_value(path, 0, local, visiting),
                    Binding::Unknown => None,
                };
            }
            if name == "default" {
                return None;
            }
            let mut values = Vec::new();
            for binding in file.imports.iter().filter(|b| b.local_name == "#export:*") {
                if let Some(value) = self.export_value(&binding.resolved_path, name, visiting) {
                    if !values.contains(&value) {
                        values.push(value);
                    }
                }
            }
            if values.len() == 1 {
                values.pop()
            } else {
                None
            }
        })();
        visiting.remove(&key);
        result
    }
    fn expression(
        &self,
        path: &Path,
        scope: usize,
        parts: &[String],
        position: usize,
        visiting: &mut BTreeSet<(PathBuf, usize, String)>,
    ) -> Option<Value> {
        let mut value = self.lookup(path, scope, parts.first()?, position, visiting)?;
        for member in &parts[1..] {
            value = match value {
                Value::Module(p) => self.export_value(&p, member, visiting)?,
                Value::ModulePrefix(p, mut prefix) => {
                    if prefix.first() != Some(member) {
                        return None;
                    }
                    prefix.remove(0);
                    if prefix.is_empty() {
                        Value::Module(p)
                    } else {
                        Value::ModulePrefix(p, prefix)
                    }
                }
                Value::Class(ref p, ref name) | Value::Instance(ref p, ref name) => {
                    let target = self.files.get(p)?;
                    let idx = *target.class_scopes.get(name)?;
                    if !target.scopes[idx].bindings.contains_key(member) {
                        return None;
                    }
                    let resolved = self.lookup(p, idx, member, usize::MAX, visiting)?;
                    if target.javascript {
                        if let Value::Function(id) = &resolved {
                            let is_static = target.static_methods.contains(id);
                            if is_static != matches!(value, Value::Class(_, _)) {
                                return None;
                            }
                        }
                    }
                    resolved
                }
                _ => return None,
            }
        }
        Some(value)
    }
}
fn text<'a>(node: Node, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("")
}
fn parts(node: Node, source: &str) -> Option<Vec<String>> {
    match node.kind() {
        "identifier"
        | "this"
        | "property_identifier"
        | "type_identifier"
        | "shorthand_property_identifier" => Some(vec![text(node, source).to_owned()]),
        "attribute" | "member_expression" => {
            let mut names = parts(node.child_by_field_name("object")?, source)?;
            let attr = node
                .child_by_field_name("attribute")
                .or_else(|| node.child_by_field_name("property"))?;
            names.push(text(attr, source).to_owned());
            Some(names)
        }
        _ => None,
    }
}
fn add(file: &mut FileIndex, scope: usize, name: String, binding: Binding) {
    if !name.is_empty() {
        file.scopes[scope]
            .bindings
            .entry(name)
            .or_default()
            .push(binding);
    }
}
fn bind_targets(node: Node, source: &str, file: &mut FileIndex, scope: usize, binding: Binding) {
    if node.kind() == "identifier" {
        add(file, scope, text(node, source).to_owned(), binding);
        return;
    }
    if matches!(
        node.kind(),
        "attribute" | "member_expression" | "subscript" | "type"
    ) {
        return;
    }
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        bind_targets(child, source, file, scope, Binding::Unknown);
    }
}
fn parameters(
    node: Node,
    source: &str,
    file: &mut FileIndex,
    scope: usize,
    class: Option<&str>,
    path: &Path,
) {
    let mut c = node.walk();
    for param in node.named_children(&mut c) {
        let target = param
            .child_by_field_name("name")
            .or_else(|| param.child_by_field_name("pattern"))
            .unwrap_or(param);
        if matches!(
            target.kind(),
            "typed_parameter"
                | "typed_default_parameter"
                | "default_parameter"
                | "required_parameter"
                | "optional_parameter"
        ) {
            let mut pc = target.walk();
            if let Some(id) = target
                .named_children(&mut pc)
                .find(|n| n.kind() == "identifier")
            {
                bind_targets(id, source, file, scope, Binding::Unknown);
            };
        } else {
            let binding = if matches!(text(target, source), "self" | "cls") {
                class
                    .map(|n| Binding::Value(Value::Instance(path.to_path_buf(), n.to_owned())))
                    .unwrap_or(Binding::Unknown)
            } else {
                Binding::Unknown
            };
            bind_targets(target, source, file, scope, binding);
        }
    }
}
fn walk(
    node: Node,
    source: &str,
    path: &Path,
    file: &mut FileIndex,
    scope: usize,
    caller: Option<&CallableId>,
    conditional: bool,
) {
    let kind = node.kind();
    if file.javascript {
        let specs = js_specs_at_node(node, source);
        if kind == "import_statement"
            || (kind == "export_statement" && node.child_by_field_name("source").is_some())
            || (kind == "variable_declarator" && !specs.is_empty())
        {
            for spec in specs {
                for token in spec
                    .specifier
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                {
                    if !token.is_empty() {
                        file.potential_imports.insert(token.to_owned());
                    }
                }
                let binding = if conditional {
                    Binding::Unknown
                } else {
                    Binding::Import(spec.local_name.clone())
                };
                add(file, scope, spec.local_name.clone(), binding.clone());
                if let Some(export) = spec.local_name.strip_prefix("#export:") {
                    file.exports.entry(export.into()).or_default().push(binding);
                    file.references.push((scope, spec.local_name.clone()));
                }
            }
            return;
        }
        if kind == "export_statement" && scope == 0 {
            if let Some(value) = node.child_by_field_name("value") {
                if let Some(parts) = parts(value, source) {
                    file.exports
                        .entry("default".into())
                        .or_default()
                        .push(Binding::Expression {
                            parts,
                            instance: false,
                            position: node.start_byte(),
                        });
                }
            }
            let mut cursor = node.walk();
            for clause in node.named_children(&mut cursor) {
                if clause.kind() == "export_clause" {
                    let mut items = clause.walk();
                    for item in clause.named_children(&mut items) {
                        if let Some(name) = item.child_by_field_name("name") {
                            let alias = item.child_by_field_name("alias").unwrap_or(name);
                            file.exports
                                .entry(text(alias, source).into())
                                .or_default()
                                .push(Binding::Expression {
                                    parts: vec![text(name, source).into()],
                                    instance: false,
                                    position: node.start_byte(),
                                });
                        }
                    }
                }
            }
        }
        if matches!(
            kind,
            "interface_declaration" | "type_alias_declaration" | "enum_declaration"
        ) {
            if let Some(name) = node.child_by_field_name("name") {
                add(file, scope, text(name, source).into(), Binding::Unknown);
                declaration_export(node, text(name, source), Binding::Unknown, file, scope);
            }
            return;
        }
    }
    if kind == "identifier" || kind == "shorthand_property_identifier" {
        let attribute_name = node
            .parent()
            .map(|p| {
                p.child_by_field_name("attribute") == Some(node)
                    || p.child_by_field_name("property") == Some(node)
            })
            .unwrap_or(false);
        if !attribute_name {
            file.references.push((scope, text(node, source).to_owned()));
        }
    }
    if kind == "as_pattern_target" {
        bind_targets(node, source, file, scope, Binding::Unknown);
    }
    if kind == "dotted_name"
        && node.parent().map(|p| p.kind()) == Some("case_pattern")
        && node.named_child_count() == 1
    {
        if let Some(name) = node.named_child(0) {
            if text(name, source) != "_" {
                bind_targets(name, source, file, scope, Binding::Unknown);
            }
        }
    }
    if matches!(kind, "class_definition" | "class_declaration")
        || (kind == "class" && default_export(node))
    {
        let name_node = node.child_by_field_name("name");
        let Some(name) = name_node
            .map(|n| text(n, source).to_owned())
            .or_else(|| default_export(node).then(|| "default".into()))
        else {
            return;
        };
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if Some(child) != node.child_by_field_name("name")
                && Some(child) != node.child_by_field_name("body")
            {
                collect_references(child, source, file, scope);
            }
        }
        let q = qualify(&file.scopes[scope].qualifier, &name);
        declaration_export(
            node,
            &name,
            Binding::Value(Value::Class(path.to_path_buf(), q.clone())),
            file,
            scope,
        );
        add(
            file,
            scope,
            name,
            if conditional {
                Binding::Unknown
            } else {
                Binding::Value(Value::Class(path.to_path_buf(), q.clone()))
            },
        );
        let child = file.scopes.len();
        file.scopes.push(Scope {
            parent: Some(scope),
            qualifier: q.clone(),
            class: true,
            ..Scope::default()
        });
        file.class_scopes.insert(q, child);
        if let Some(body) = node.child_by_field_name("body") {
            walk(body, source, path, file, child, None, false);
        }
        return;
    }
    let mut function_node = node;
    let mut name_node = node.child_by_field_name("name");
    let mut is_function = matches!(
        kind,
        "function_definition"
            | "function_declaration"
            | "generator_function_declaration"
            | "method_definition"
    );
    if default_export(node) && matches!(kind, "function_expression" | "arrow_function") {
        is_function = true;
    }
    if kind == "variable_declarator" {
        if let Some(value) = node.child_by_field_name("value") {
            if matches!(
                value.kind(),
                "arrow_function" | "function" | "function_expression" | "generator_function"
            ) {
                function_node = value;
                is_function = true;
            }
        }
    }
    if is_function {
        if name_node.is_none() {
            name_node = function_node.child_by_field_name("name");
        }
        let Some(name) = name_node
            .map(|n| text(n, source).to_owned())
            .or_else(|| default_export(node).then(|| "default".into()))
        else {
            return;
        };
        let q = qualify(&file.scopes[scope].qualifier, &name);
        let id = CallableId {
            file_path: path.to_path_buf(),
            qualified_name: q.clone(),
            start_byte: function_node.start_byte(),
        };
        let parent_class = if file.scopes[scope].class {
            Some(file.scopes[scope].qualifier.clone())
        } else {
            None
        };
        file.declarations.insert(
            id.clone(),
            Callable {
                id: id.clone(),
                file_path: path.to_path_buf(),
                name: name.clone(),
                qualified_name: q.clone(),
                start_byte: function_node.start_byte(),
                end_byte: function_node.end_byte(),
                start_line: function_node.start_position().row + 1,
                end_line: function_node.end_position().row + 1,
                parent_class: parent_class.clone(),
            },
        );
        let js_static = file.javascript && node.kind() == "method_definition" && {
            let mut cursor = node.walk();
            node.children(&mut cursor)
                .any(|child| child.kind() == "static")
        };
        if js_static {
            file.static_methods.insert(id.clone());
        }
        declaration_export(
            node,
            &name,
            Binding::Value(Value::Function(id.clone())),
            file,
            scope,
        );
        add(
            file,
            scope,
            name,
            if conditional {
                Binding::Unknown
            } else {
                Binding::Value(Value::Function(id.clone()))
            },
        );
        let child = file.scopes.len();
        file.scopes.push(Scope {
            parent: Some(scope),
            qualifier: q,
            ..Scope::default()
        });
        if let Some(params) = function_node.child_by_field_name("parameters") {
            let mut cursor = params.walk();
            for param in params.named_children(&mut cursor) {
                if param.kind() == "identifier" {
                    continue;
                }
                let bound = param
                    .child_by_field_name("name")
                    .or_else(|| param.child_by_field_name("pattern"));
                let mut pc = param.walk();
                for part in param.named_children(&mut pc) {
                    if Some(part) != bound && !(bound.is_none() && part.kind() == "identifier") {
                        collect_references(part, source, file, scope);
                    }
                }
            }
        }
        if let Some(annotation) = function_node.child_by_field_name("return_type") {
            collect_references(annotation, source, file, scope);
        }
        let is_static = node
            .parent()
            .filter(|p| p.kind() == "decorated_definition")
            .map(|parent| {
                let mut cursor = parent.walk();
                parent.named_children(&mut cursor).any(|n| {
                    n.kind() == "decorator"
                        && matches!(text(n, source), "@staticmethod" | "@builtins.staticmethod")
                })
            })
            .unwrap_or(false);
        if let Some(params) = function_node.child_by_field_name("parameters") {
            parameters(
                params,
                source,
                file,
                child,
                if is_static {
                    None
                } else {
                    parent_class.as_deref()
                },
                path,
            );
        }
        if let Some(param) = function_node.child_by_field_name("parameter") {
            bind_targets(param, source, file, child, Binding::Unknown);
        }
        if let Some(class) = parent_class {
            add(
                file,
                child,
                "this".into(),
                Binding::Value(if js_static {
                    Value::Class(path.to_path_buf(), class)
                } else {
                    Value::Instance(path.to_path_buf(), class)
                }),
            );
        }
        if let Some(body) = function_node.child_by_field_name("body") {
            walk(body, source, path, file, child, Some(&id), false);
        }
        return;
    }
    if matches!(kind, "import_statement" | "import_from_statement") {
        let module = node.child_by_field_name("module_name");
        for token in text(node, source).split(|c: char| !c.is_alphanumeric() && c != '_') {
            if !token.is_empty() {
                file.potential_imports.insert(token.to_owned());
            }
        }
        let mut c = node.walk();
        for child in node.named_children(&mut c) {
            if Some(child) == module {
                continue;
            }
            if kind == "import_statement" && child.kind() == "dotted_name" {
                let names: Vec<_> = text(child, source).split('.').map(String::from).collect();
                if names.len() > 1 {
                    file.module_prefixes
                        .insert(names[0].clone(), names[1..].to_vec());
                }
            }
            let name = match child.kind() {
                "dotted_name" => text(child, source)
                    .split('.')
                    .next()
                    .unwrap_or("")
                    .to_owned(),
                "aliased_import" => child
                    .child_by_field_name("alias")
                    .or_else(|| child.child_by_field_name("name"))
                    .map(|n| text(n, source).to_owned())
                    .unwrap_or_default(),
                _ => continue,
            };
            add(
                file,
                scope,
                name.clone(),
                if conditional {
                    Binding::Unknown
                } else {
                    Binding::Import(name)
                },
            );
        }
        return;
    }
    if matches!(
        kind,
        "assignment"
            | "augmented_assignment"
            | "named_expression"
            | "variable_declarator"
            | "assignment_expression"
    ) {
        let left = node
            .child_by_field_name("left")
            .or_else(|| node.child_by_field_name("name"));
        let right = node
            .child_by_field_name("right")
            .or_else(|| node.child_by_field_name("value"));
        let value = right
            .and_then(|r| {
                let instance = matches!(r.kind(), "call" | "call_expression" | "new_expression");
                let target = if instance {
                    r.child_by_field_name("function")
                        .or_else(|| r.child_by_field_name("constructor"))?
                } else {
                    r
                };
                Some(Binding::Expression {
                    parts: parts(target, source)?,
                    instance,
                    position: node.end_byte(),
                })
            })
            .unwrap_or(Binding::Unknown);
        if file.javascript && scope == 0 {
            if let Some(left) = left {
                if left.kind() == "identifier" {
                    declaration_export(node, text(left, source), value.clone(), file, scope);
                }
                if let Some(names) = parts(left, source) {
                    let exported = if names.len() == 2 && names[0] == "exports" {
                        Some(names[1].clone())
                    } else if names.len() == 3 && names[0] == "module" && names[1] == "exports" {
                        Some(names[2].clone())
                    } else if names == ["module", "exports"] {
                        Some("default".into())
                    } else {
                        None
                    };
                    if let Some(exported) = exported {
                        let root = names[0].clone();
                        file.commonjs_roots.insert(exported.clone(), root.clone());
                        let is_object = exported == "default"
                            && right.map(|r| r.kind() == "object").unwrap_or(false);
                        let exported_value = if conditional {
                            Binding::Unknown
                        } else if is_object {
                            Binding::Value(Value::Module(path.to_path_buf()))
                        } else {
                            value.clone()
                        };
                        file.exports
                            .entry(exported)
                            .or_default()
                            .push(exported_value);
                        if is_object {
                            let object = right.unwrap();
                            let mut cursor = object.walk();
                            for member in object.named_children(&mut cursor) {
                                let pair = if member.kind() == "pair" {
                                    member
                                        .child_by_field_name("key")
                                        .zip(member.child_by_field_name("value"))
                                } else if member.kind() == "shorthand_property_identifier" {
                                    Some((member, member))
                                } else {
                                    None
                                };
                                if let Some((key, value)) = pair {
                                    let binding = if conditional {
                                        Binding::Unknown
                                    } else {
                                        parts(value, source)
                                            .map(|parts| Binding::Expression {
                                                parts,
                                                instance: false,
                                                position: node.start_byte(),
                                            })
                                            .unwrap_or(Binding::Unknown)
                                    };
                                    file.exports
                                        .entry(text(key, source).into())
                                        .or_default()
                                        .push(binding);
                                    file.commonjs_roots
                                        .insert(text(key, source).into(), root.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(left) = left {
            bind_targets(
                left,
                source,
                file,
                scope,
                if conditional { Binding::Unknown } else { value },
            );
        }
    }
    if matches!(kind, "for_statement" | "for_in_statement") {
        if let Some(left) = node.child_by_field_name("left") {
            bind_targets(left, source, file, scope, Binding::Unknown);
        }
    }
    if matches!(
        kind,
        "global_statement" | "nonlocal_statement" | "delete_statement"
    ) {
        let mut c = node.walk();
        for child in node.named_children(&mut c) {
            bind_targets(child, source, file, scope, Binding::Unknown);
        }
    }
    if matches!(
        kind,
        "lambda"
            | "lambda_expression"
            | "list_comprehension"
            | "set_comprehension"
            | "dictionary_comprehension"
            | "generator_expression"
    ) {
        // These introduce distinct binding/evaluation scopes. Exclude their
        // calls until modeled rather than attribute them to an outer function.
        return;
    }
    if matches!(kind, "call" | "call_expression") {
        if let (Some(caller), Some(target)) = (caller, node.child_by_field_name("function")) {
            if let Some(parts) = parts(target, source) {
                file.sites.push(Site {
                    caller: caller.clone(),
                    scope,
                    parts,
                    start: node.start_byte(),
                });
            }
        }
    }
    let conditional = conditional
        || matches!(
            kind,
            "if_statement"
                | "try_statement"
                | "for_statement"
                | "while_statement"
                | "match_statement"
        );
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        walk(child, source, path, file, scope, caller, conditional);
    }
}
fn qualify(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.into()
    } else {
        format!("{parent}.{name}")
    }
}

fn collect_references(node: Node, source: &str, file: &mut FileIndex, scope: usize) {
    if node.kind() == "identifier" || node.kind() == "type_identifier" {
        let attribute_name = node
            .parent()
            .map(|p| {
                p.child_by_field_name("attribute") == Some(node)
                    || p.child_by_field_name("property") == Some(node)
            })
            .unwrap_or(false);
        if !attribute_name {
            file.references.push((scope, text(node, source).to_owned()));
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_references(child, source, file, scope);
    }
}

/// Static JS/TS binding before the repository resolver supplies its file path.
/// `#export:` identifies re-exports; `#require:` distinguishes CommonJS from an
/// ES namespace import, and `#type:` marks a type-only import.
#[derive(Debug, Clone)]
pub struct JsImportSpec {
    pub local_name: String,
    pub specifier: String,
    pub imported_symbol: Option<String>,
}

pub fn js_import_specs(tree: &Tree, source: &str) -> Vec<JsImportSpec> {
    fn visit(node: Node, source: &str, out: &mut Vec<JsImportSpec>) {
        out.extend(js_specs_at_node(node, source));
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            visit(child, source, out);
        }
    }
    let mut result = Vec::new();
    visit(tree.root_node(), source, &mut result);
    result
}
fn js_string(node: Node, source: &str) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }
    let raw = text(node, source);
    Some(raw.get(1..raw.len().checked_sub(1)?)?.to_owned())
}
fn require_target(node: Node, source: &str) -> Option<(String, Option<String>)> {
    if node.kind() == "member_expression" {
        let (mut module, _) = require_target(node.child_by_field_name("object")?, source)?;
        let member = node.child_by_field_name("property")?;
        if member.kind() != "property_identifier" {
            return None;
        }
        return Some((
            std::mem::take(&mut module),
            Some(text(member, source).into()),
        ));
    }
    if node.kind() != "call_expression"
        || text(node.child_by_field_name("function")?, source) != "require"
    {
        return None;
    }
    let args = node.child_by_field_name("arguments")?;
    if args.named_child_count() != 1 {
        return None;
    }
    Some((js_string(args.named_child(0)?, source)?, None))
}
fn js_specs_at_node(node: Node, source: &str) -> Vec<JsImportSpec> {
    let mut result = Vec::new();
    let mut push = |local: String, specifier: String, symbol: Option<String>| {
        result.push(JsImportSpec {
            local_name: local,
            specifier,
            imported_symbol: symbol,
        })
    };
    if matches!(node.kind(), "import_statement" | "export_statement") {
        let Some(specifier) = node
            .child_by_field_name("source")
            .and_then(|s| js_string(s, source))
        else {
            return result;
        };
        let exporting = node.kind() == "export_statement";
        let type_only = text(node, source).trim_start().starts_with(if exporting {
            "export type "
        } else {
            "import type "
        });
        let mut clauses = node.walk();
        for clause in node.named_children(&mut clauses) {
            if matches!(clause.kind(), "import_clause" | "export_clause") {
                let mut children = clause.walk();
                for child in clause.named_children(&mut children) {
                    if child.kind() == "identifier" {
                        push(
                            text(child, source).into(),
                            specifier.clone(),
                            Some(
                                if type_only {
                                    "#type:default"
                                } else {
                                    "default"
                                }
                                .into(),
                            ),
                        );
                    } else if child.kind() == "namespace_import" {
                        if let Some(local) = child.named_child(0) {
                            push(
                                text(local, source).into(),
                                specifier.clone(),
                                if type_only {
                                    Some("#type:*".into())
                                } else {
                                    None
                                },
                            );
                        }
                    } else if child.kind() == "named_imports" {
                        let mut specs = child.walk();
                        for item in child.named_children(&mut specs) {
                            if let Some(name) = item.child_by_field_name("name") {
                                let local = item.child_by_field_name("alias").unwrap_or(name);
                                let symbol = format!(
                                    "{}{}",
                                    if type_only
                                        || text(item, source).trim_start().starts_with("type ")
                                    {
                                        "#type:"
                                    } else {
                                        ""
                                    },
                                    text(name, source)
                                );
                                push(text(local, source).into(), specifier.clone(), Some(symbol));
                            }
                        }
                    } else if child.kind() == "export_specifier" {
                        if let Some(name) = child.child_by_field_name("name") {
                            let exported = child.child_by_field_name("alias").unwrap_or(name);
                            push(
                                format!("#export:{}", text(exported, source)),
                                specifier.clone(),
                                Some(format!(
                                    "{}{}",
                                    if type_only { "#type:" } else { "" },
                                    text(name, source)
                                )),
                            );
                        }
                    }
                }
            } else if clause.kind() == "namespace_export" {
                if let Some(name) = clause.named_child(0) {
                    push(
                        format!("#export:{}", text(name, source)),
                        specifier.clone(),
                        None,
                    );
                }
            }
        }
        if exporting && result.is_empty() && !type_only {
            result.push(JsImportSpec {
                local_name: "#export:*".into(),
                specifier,
                imported_symbol: None,
            });
        }
    } else if node.kind() == "variable_declarator" {
        let Some(value) = node.child_by_field_name("value") else {
            return result;
        };
        let Some((specifier, member)) = require_target(value, source) else {
            return result;
        };
        let Some(name) = node.child_by_field_name("name") else {
            return result;
        };
        if name.kind() == "identifier" {
            push(
                text(name, source).into(),
                specifier,
                Some(format!("#require:{}", member.unwrap_or_default())),
            );
        } else if name.kind() == "object_pattern" && member.is_none() {
            let mut cursor = name.walk();
            for item in name.named_children(&mut cursor) {
                if item.kind() == "shorthand_property_identifier_pattern" {
                    push(
                        text(item, source).into(),
                        specifier.clone(),
                        Some(format!("#require:{}", text(item, source))),
                    );
                } else if item.kind() == "pair_pattern" {
                    if let (Some(key), Some(value)) = (
                        item.child_by_field_name("key"),
                        item.child_by_field_name("value"),
                    ) {
                        if value.kind() == "identifier" {
                            push(
                                text(value, source).into(),
                                specifier.clone(),
                                Some(format!("#require:{}", text(key, source))),
                            );
                        }
                    }
                }
            }
        }
    }
    result
}

fn default_export(node: Node) -> bool {
    node.parent()
        .map(|parent| {
            if parent.kind() != "export_statement" {
                return false;
            }
            let mut cursor = parent.walk();
            parent.children(&mut cursor).any(|c| c.kind() == "default")
        })
        .unwrap_or(false)
}
fn declaration_export(
    node: Node,
    name: &str,
    _binding: Binding,
    file: &mut FileIndex,
    scope: usize,
) {
    if !file.javascript || scope != 0 {
        return;
    }
    let parent = node.parent();
    let export = parent
        .filter(|p| p.kind() == "export_statement")
        .or_else(|| {
            parent
                .and_then(|p| p.parent())
                .filter(|p| p.kind() == "export_statement")
        });
    if let Some(export) = export {
        let mut cursor = export.walk();
        let is_default = export.children(&mut cursor).any(|c| c.kind() == "default");
        file.exports
            .entry(if is_default {
                "default".into()
            } else {
                name.into()
            })
            .or_default()
            .push(Binding::Expression {
                parts: vec![name.into()],
                instance: false,
                position: node.start_byte(),
            });
    }
}
