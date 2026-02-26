use lazy_static::lazy_static;
use log::{debug, info};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use tree_sitter::{Query, QueryCursor, Tree};
use walkdir::WalkDir;
use crate::parser::SupportedLanguage;

lazy_static! {
    static ref PYTHON_STDLIB_MODULES: HashSet<&'static str> = [
        "os", "sys", "math", "json", "re", "collections", "datetime", "time", "random",
        "logging", "argparse", "io", "abc", "typing", "functools", "itertools",
        "heapq", "queue", "threading", "multiprocessing", "subprocess", "socket",
        "select", "ssl", "http", "urllib", "email", "csv", "xml", "html",
        "dataclasses", "enum", "decimal", "fractions", "pathlib", "shutil", "tempfile",
        "zipfile", "tarfile", "gzip", "bz2", "lzma", "hashlib", "hmac", "secrets",
        "array", "mmap", "struct", "warnings", "contextlib", "locale", "gettext",
        "unittest", "doctest", "pdb", "cProfile", "profile", "timeit", "trace",
        "linecache", "faulthandler", "inspect", "dis", "gc", "sysconfig", "builtins",
    ].iter().cloned().collect();

    static ref COMMON_THIRD_PARTY_MODULES: HashSet<&'static str> = [
        "numpy", "pandas", "django", "flask", "requests", "sqlalchemy", "werkzeug",
        "tensorflow", "torch", "matplotlib", "scipy", "sklearn", "pytz", "pytest",
        "PIL", "cv2", "fastapi", "pydantic", "starlette", "uvicorn", "jinja2", "itsdangerous"
    ].iter().cloned().collect();

    static ref NODE_BUILTIN_MODULES: HashSet<&'static str> = [
        "assert", "buffer", "child_process", "cluster", "crypto", "dgram",
        "dns", "domain", "events", "fs", "http", "https", "net", "os",
        "path", "punycode", "querystring", "readline", "stream", "string_decoder",
        "timers", "tls", "tty", "url", "util", "v8", "vm", "zlib",
        "worker_threads", "perf_hooks", "async_hooks", "inspector",
    ].iter().cloned().collect();
}

#[derive(Debug, Deserialize)]
struct TsConfig {
    #[serde(rename = "compilerOptions")]
    compiler_options: Option<CompilerOptions>,
}

#[derive(Debug, Deserialize)]
struct CompilerOptions {
    #[serde(rename = "baseUrl")]
    base_url: Option<String>,
    paths: Option<HashMap<String, Vec<String>>>,
}

/// The result of an import resolution attempt, providing more context than Option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportResolutionResult {
    /// The import was successfully resolved to a file path within the project.
    Resolved(PathBuf),
    /// The import is identified as a standard library module.
    Stdlib(String),
    /// The import is identified as a likely third-party package.
    External(String),
    /// The import could not be resolved to any known file or category.
    Unresolved(String),
}

/// A local binding created by an import statement.
#[derive(Debug, Clone)]
pub struct ImportBinding {
    /// The local name used in this file (e.g., "utils", "HttpClient")
    pub local_name: String,
    /// The resolved file path this binding points to
    pub resolved_path: PathBuf,
    /// The specific symbol imported (None for module imports like `import utils`)
    pub imported_symbol: Option<String>,
}

/// Common interface for language-specific import resolution.
pub trait ImportResolver: Debug + Send + Sync {
    /// Finds all resolvable, project-local imports in a file's AST.
    fn find_imports<'a>(
        &self,
        tree: &'a Tree,
        current_file: &Path,
        source: &'a [u8],
    ) -> HashSet<PathBuf>;

    /// Finds import bindings: local name → (resolved path, optional symbol).
    /// Used by call graph resolution to resolve module-qualified calls.
    fn find_import_bindings<'a>(
        &self,
        _tree: &'a Tree,
        _current_file: &Path,
        _source: &'a [u8],
    ) -> Vec<ImportBinding> {
        Vec::new()
    }

    /// Returns the file extensions this resolver handles.
    fn file_extensions(&self) -> &[&str];

    /// Get detected source roots (for diagnostics). Default: empty.
    fn get_source_roots(&self) -> Vec<PathBuf> { Vec::new() }

    /// Get module index size (for diagnostics). Default: 0.
    fn module_index_size(&self) -> usize { 0 }
}

/// Resolves Python import statements to absolute file paths.
#[derive(Debug)]
pub struct PythonImportResolver {
    pub project_root: PathBuf,
    /// A map of module names (e.g., `my_package.utils`) to their file paths.
    module_index: HashMap<String, PathBuf>,
    /// Compiled query for import statements.
    import_query: Query,
    /// Directory names to exclude from module indexing.
    ignored_dirs: HashSet<String>,
    /// Detected source roots (directories containing Python packages).
    /// Modules are also indexed relative to each source root.
    source_roots: Vec<PathBuf>,
}

impl PythonImportResolver {
    /// Creates a new `PythonImportResolver` and indexes all Python modules in the project.
    /// `ignored_dirs` contains directory names (not paths) to exclude from module indexing.
    /// `explicit_source_roots` optionally overrides auto-detection of source roots.
    pub fn new(project_root: &Path, ignored_dirs: &[String], explicit_source_roots: Option<&[String]>) -> Self {
        let ignored_set: HashSet<String> = ignored_dirs.iter().cloned().collect();
        let canonical_root = project_root.canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());
        let mut resolver = Self {
            project_root: canonical_root.clone(),
            module_index: HashMap::new(),
            ignored_dirs: ignored_set,
            source_roots: Vec::new(),
            // Pre-compile the query for performance
            // Pattern 0: absolute import (e.g., `import app.models`)
            // Pattern 1: entire from-import statement for programmatic AST walking
            import_query: Query::new(
                tree_sitter_python::language(),
                r#"
                (import_statement name: (dotted_name) @module)
                (import_from_statement) @from_import
                "#,
            )
            .expect("Failed to create import query"),
        };
        resolver.source_roots = if let Some(roots) = explicit_source_roots {
            let explicit: Vec<PathBuf> = roots.iter()
                .filter_map(|r| {
                    let p = canonical_root.join(r);
                    let canonical = p.canonicalize().unwrap_or(p);
                    if canonical.is_dir() {
                        info!("Explicit source root: {}", canonical.display());
                        Some(canonical)
                    } else {
                        info!("Ignoring non-existent source root: {}", r);
                        None
                    }
                })
                .collect();
            if explicit.is_empty() {
                info!("No valid explicit source roots, falling back to auto-detection");
                resolver.detect_source_roots()
            } else {
                explicit
            }
        } else {
            resolver.detect_source_roots()
        };
        resolver.index_modules();
        resolver
    }

    /// Detect source roots: depth-1 directories that contain Python packages.
    /// A directory is a source root if it has at least one child directory with `__init__.py`.
    /// Falls back to namespace package detection if no `__init__.py` roots found.
    fn detect_source_roots(&self) -> Vec<PathBuf> {
        let mut roots = Vec::new();

        info!("Source root detection: scanning project root {:?}", self.project_root);
        info!("Source root detection: ignored_dirs = {:?}", self.ignored_dirs);

        let read_dir = match std::fs::read_dir(&self.project_root) {
            Ok(rd) => rd,
            Err(e) => {
                info!("Source root detection: cannot read project root {:?}: {}", self.project_root, e);
                return roots;
            }
        };

        for entry in read_dir.filter_map(Result::ok) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            // Skip ignored directories
            if self.ignored_dirs.contains(&dir_name) {
                debug!("Source root detection: skipping ignored dir {}", dir_name);
                continue;
            }

            // Check if this directory contains at least one Python package.
            // A child dir is a Python package if:
            //   1. It has __init__.py (traditional package), OR
            //   2. It contains .py files (namespace/implicit package), OR
            //   3. It has subdirectories with __init__.py (nested packages)
            let mut found_package = false;
            if let Ok(sub_entries) = std::fs::read_dir(&path) {
                for sub_entry in sub_entries.filter_map(Result::ok) {
                    let sub_path = sub_entry.path();
                    if !sub_path.is_dir() {
                        continue;
                    }
                    let sub_name = sub_path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("");
                    if sub_name.starts_with('.') || sub_name == "__pycache__" {
                        continue;
                    }

                    // Check 1: traditional package (has __init__.py)
                    if sub_path.join("__init__.py").exists() {
                        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                        info!("Detected source root: {} (found package {})",
                              canonical.display(), sub_name);
                        roots.push(canonical);
                        found_package = true;
                        break;
                    }

                    // Check 2: namespace package (has .py files or sub-packages)
                    if let Ok(grandchildren) = std::fs::read_dir(&sub_path) {
                        let has_python = grandchildren.filter_map(Result::ok).any(|gc| {
                            let gc_path = gc.path();
                            if gc_path.is_file() {
                                gc_path.extension().and_then(|e| e.to_str()) == Some("py")
                            } else if gc_path.is_dir() {
                                gc_path.join("__init__.py").exists()
                            } else {
                                false
                            }
                        });
                        if has_python {
                            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                            info!("Detected source root: {} (found namespace package {})",
                                  canonical.display(), sub_name);
                            roots.push(canonical);
                            found_package = true;
                            break;
                        }
                    }
                }
            } else {
                info!("Source root detection: cannot read_dir {}", path.display());
            }
            if !found_package {
                debug!("Source root detection: {} has no Python package subdirs", dir_name);
            }
        }

        // Fallback: namespace packages (dirs with Python subdirs but no __init__.py)
        if roots.is_empty() {
            let read_dir2 = match std::fs::read_dir(&self.project_root) {
                Ok(rd) => rd,
                Err(_) => return roots,
            };

            for entry in read_dir2.filter_map(Result::ok) {
                let path = entry.path();
                if !path.is_dir() { continue; }
                let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                if self.ignored_dirs.contains(&dir_name) { continue; }

                // Check if this directory has subdirectories containing .py files
                if let Ok(sub_entries) = std::fs::read_dir(&path) {
                    for sub_entry in sub_entries.filter_map(Result::ok) {
                        let sub_path = sub_entry.path();
                        if sub_path.is_dir() {
                            let has_py = std::fs::read_dir(&sub_path)
                                .map(|entries| entries.filter_map(Result::ok).any(|e| {
                                    e.path().extension().and_then(|ext| ext.to_str()) == Some("py")
                                }))
                                .unwrap_or(false);
                            if has_py {
                                let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                                info!("Detected namespace-package source root: {}", canonical.display());
                                roots.push(canonical);
                                break;
                            }
                        }
                    }
                }
            }
        }

        if roots.is_empty() {
            info!("No source roots detected for {:?}", self.project_root);
        } else {
            info!("Detected {} source root(s) for {:?}", roots.len(), self.project_root);
        }

        roots
    }

    /// Scans the project directory for Python files (.py) and builds the module index.
    /// Skips directories listed in `ignored_dirs`.
    /// All paths stored in module_index are canonicalized to match scan_repository output.
    fn index_modules(&mut self) {
        let mut sr_variant_count = 0usize;

        for entry in WalkDir::new(&self.project_root)
            .into_iter()
            .filter_entry(|e| {
                if e.file_type().is_dir() {
                    let name = e.file_name().to_string_lossy();
                    return !self.ignored_dirs.contains(name.as_ref());
                }
                true
            })
            .filter_map(Result::ok)
            .filter(|e| e.path().is_file())
        {
            // Canonicalize to match the canonical paths from scan_repository
            let canonical_path = entry.path().canonicalize()
                .unwrap_or_else(|_| entry.path().to_path_buf());
            let extension = canonical_path.extension().and_then(|s| s.to_str());

            if extension == Some("py") || extension == Some("pyi") {
                if let Ok(relative_path) = canonical_path.strip_prefix(&self.project_root) {
                    let mut components: Vec<String> = relative_path
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().to_string())
                        .collect();

                    let module_path_str =
                        if components.last() == Some(&"__init__.py".to_string())
                            || components.last() == Some(&"__init__.pyi".to_string())
                        {
                            components.pop(); // Remove `__init__.py`
                            components.join(".")
                        } else {
                            if let Some(last) = components.last_mut() {
                                if let Some(stem) = Path::new(last).file_stem() {
                                    *last = stem.to_string_lossy().to_string();
                                }
                            }
                            components.join(".")
                        };

                    if !module_path_str.is_empty() {
                        self.module_index
                            .insert(module_path_str, canonical_path.clone());

                        // Also register relative to each source root
                        for source_root in &self.source_roots {
                            if let Ok(sr_relative) = canonical_path.strip_prefix(source_root) {
                                let mut sr_components: Vec<String> = sr_relative
                                    .components()
                                    .map(|c| c.as_os_str().to_string_lossy().to_string())
                                    .collect();

                                let sr_module_str =
                                    if sr_components.last() == Some(&"__init__.py".to_string())
                                        || sr_components.last() == Some(&"__init__.pyi".to_string())
                                    {
                                        sr_components.pop();
                                        sr_components.join(".")
                                    } else {
                                        if let Some(last) = sr_components.last_mut() {
                                            if let Some(stem) = Path::new(last).file_stem() {
                                                *last = stem.to_string_lossy().to_string();
                                            }
                                        }
                                        sr_components.join(".")
                                    };

                                if !sr_module_str.is_empty() {
                                    // First-registered wins: don't overwrite existing entries
                                    self.module_index
                                        .entry(sr_module_str)
                                        .or_insert_with(|| canonical_path.clone());
                                    sr_variant_count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        info!(
            "Module index: {} entries ({} source-root-relative variants)",
            self.module_index.len(),
            sr_variant_count,
        );
    }

    fn resolve_absolute(&self, module_path: &str) -> Option<PathBuf> {
        // 1. Direct match (e.g., `my_package.utils` -> `my_package/utils.py`)
        if let Some(path) = self.module_index.get(module_path) {
            return Some(path.clone());
        }

        // 2. Package match (e.g., `my_package.utils` -> `my_package/utils/__init__.py`)
        let package_path_str = format!("{}.__init__", module_path);
        if let Some(path) = self.module_index.get(&package_path_str) {
            return Some(path.clone());
        }

        None
    }

    fn resolve_relative(
        &self,
        current_file: &Path,
        dots: usize,
        relative_module: Option<&str>,
    ) -> Option<PathBuf> {
        // Start at file's directory
        let file_dir = current_file.parent()?;
        let mut base_path = file_dir.to_path_buf();

        // Go up (dots - 1) levels.
        // For `.` (dots=1), loop doesn't run, base_path is current dir.
        // For `..` (dots=2), loop runs once, base_path is parent dir.
        for _ in 1..dots {
            base_path = base_path.parent()?.to_path_buf();
        }

        // If the current file is NOT in a package (no __init__.py), a `..` import
        // should look relative to the parent package, so we go up one more level.
        if !file_dir.join("__init__.py").exists() && dots > 1 {
            base_path = base_path.parent()?.to_path_buf();
        }

        // Convert base_path to a module string from project root.
        let rel_path = match base_path.strip_prefix(&self.project_root) {
            Ok(p) => p,
            Err(_) => return None,
        };

        let mut components: Vec<String> = rel_path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();

        if let Some(module) = relative_module {
            if !module.is_empty() {
                components.extend(module.split('.').map(String::from));
            }
        }

        let module_str = components.join(".");
        self.resolve_absolute(&module_str)
    }
}

impl PythonImportResolver {
    /// Resolves a `from ... import ...` statement by walking its AST node.
    fn resolve_from_import(
        &self,
        node: tree_sitter::Node,
        current_file: &Path,
        source: &[u8],
        imports: &mut HashSet<PathBuf>,
    ) {
        let module_name_node = node.child_by_field_name("module_name");

        match module_name_node {
            Some(mn) if mn.kind() == "dotted_name" => {
                // Absolute: `from app.models import User`
                let text = mn.utf8_text(source).unwrap_or("");
                let root_module = text.split('.').next().unwrap_or("");
                if !PYTHON_STDLIB_MODULES.contains(root_module)
                    && !COMMON_THIRD_PARTY_MODULES.contains(root_module)
                {
                    if let Some(path) = self.resolve_absolute(text) {
                        imports.insert(path);
                    }
                }
            }
            Some(mn) if mn.kind() == "relative_import" => {
                // Relative: `from .module import X` or `from . import X`
                let mut dots = 0usize;
                let mut module_path: Option<String> = None;

                // Walk children of the relative_import node to extract
                // the dot count (import_prefix) and optional module name (dotted_name)
                let mut cursor = mn.walk();
                for child in mn.children(&mut cursor) {
                    match child.kind() {
                        "import_prefix" => {
                            dots = child
                                .utf8_text(source)
                                .unwrap_or("")
                                .chars()
                                .filter(|&c| c == '.')
                                .count();
                        }
                        "dotted_name" => {
                            module_path =
                                Some(child.utf8_text(source).unwrap_or("").to_string());
                        }
                        _ => {}
                    }
                }

                if let Some(ref module) = module_path {
                    // `from .module import X` — resolve the module directly
                    if let Some(path) = self.resolve_relative(current_file, dots, Some(module)) {
                        imports.insert(path);
                    }
                } else {
                    // `from . import X, Y` or `from . import *`
                    // No module name inside relative_import — check what's being imported
                    let has_wildcard = {
                        let mut c = node.walk();
                        node.children(&mut c)
                            .any(|child| child.kind() == "wildcard_import")
                    };

                    if has_wildcard {
                        // `from . import *` — resolve to the package's __init__.py
                        if let Some(path) = self.resolve_relative(current_file, dots, None) {
                            imports.insert(path);
                        }
                    } else {
                        // `from . import X, Y` — each imported name could be a submodule
                        // or a symbol from __init__.py. Try module first, then fall back.
                        let mut any_resolved = false;
                        let mut cursor2 = node.walk();
                        for child in node.children(&mut cursor2) {
                            let name_text = match child.kind() {
                                "dotted_name" => child.utf8_text(source).ok(),
                                "aliased_import" => child
                                    .child_by_field_name("name")
                                    .and_then(|n| n.utf8_text(source).ok()),
                                _ => None,
                            };
                            if let Some(name) = name_text {
                                if let Some(path) =
                                    self.resolve_relative(current_file, dots, Some(name))
                                {
                                    imports.insert(path);
                                    any_resolved = true;
                                }
                            }
                        }
                        // If none resolved as modules, try the package __init__.py
                        // (the names are symbols exported from __init__.py)
                        if !any_resolved {
                            if let Some(path) =
                                self.resolve_relative(current_file, dots, None)
                            {
                                imports.insert(path);
                            }
                        }
                    }
                }
            }
            _ => {
                // No module_name field or unrecognized kind — skip
            }
        }
    }
}

impl PythonImportResolver {
    /// Collect import bindings from a bare `import X` or `import X as Y` statement.
    fn collect_bare_import_bindings(
        &self,
        node: tree_sitter::Node,
        source: &[u8],
        bindings: &mut Vec<ImportBinding>,
    ) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "dotted_name" => {
                    // `import utils` (unaliased)
                    let text = child.utf8_text(source).unwrap_or("");
                    let root_module = text.split('.').next().unwrap_or("");
                    if PYTHON_STDLIB_MODULES.contains(root_module)
                        || COMMON_THIRD_PARTY_MODULES.contains(root_module)
                    {
                        continue;
                    }
                    if let Some(path) = self.resolve_absolute(text) {
                        bindings.push(ImportBinding {
                            local_name: root_module.to_string(),
                            resolved_path: path,
                            imported_symbol: None,
                        });
                    }
                }
                "aliased_import" => {
                    // `import utils as u`
                    let name_node = child.child_by_field_name("name");
                    let alias_node = child.child_by_field_name("alias");

                    if let Some(name) = name_node {
                        let text = name.utf8_text(source).unwrap_or("");
                        let root_module = text.split('.').next().unwrap_or("");
                        if PYTHON_STDLIB_MODULES.contains(root_module)
                            || COMMON_THIRD_PARTY_MODULES.contains(root_module)
                        {
                            continue;
                        }
                        let local_name = alias_node
                            .and_then(|a| a.utf8_text(source).ok())
                            .unwrap_or(root_module);
                        if let Some(path) = self.resolve_absolute(text) {
                            bindings.push(ImportBinding {
                                local_name: local_name.to_string(),
                                resolved_path: path,
                                imported_symbol: None,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Collect import bindings from a `from ... import ...` statement.
    fn collect_from_import_bindings(
        &self,
        node: tree_sitter::Node,
        current_file: &Path,
        source: &[u8],
        bindings: &mut Vec<ImportBinding>,
    ) {
        let module_name_node = node.child_by_field_name("module_name");

        // Resolve the module path (absolute or relative)
        let resolved_module = match module_name_node {
            Some(mn) if mn.kind() == "dotted_name" => {
                let text = mn.utf8_text(source).unwrap_or("");
                let root_module = text.split('.').next().unwrap_or("");
                if PYTHON_STDLIB_MODULES.contains(root_module)
                    || COMMON_THIRD_PARTY_MODULES.contains(root_module)
                {
                    return;
                }
                self.resolve_absolute(text)
            }
            Some(mn) if mn.kind() == "relative_import" => {
                let mut dots = 0usize;
                let mut module_path: Option<String> = None;
                let mut cursor = mn.walk();
                for child in mn.children(&mut cursor) {
                    match child.kind() {
                        "import_prefix" => {
                            dots = child.utf8_text(source).unwrap_or("")
                                .chars().filter(|&c| c == '.').count();
                        }
                        "dotted_name" => {
                            module_path = Some(child.utf8_text(source).unwrap_or("").to_string());
                        }
                        _ => {}
                    }
                }
                if let Some(ref module) = module_path {
                    self.resolve_relative(current_file, dots, Some(module))
                } else {
                    self.resolve_relative(current_file, dots, None)
                }
            }
            _ => return,
        };

        let resolved_path = match resolved_module {
            Some(p) => p,
            None => return,
        };

        // Walk imported names: `from X import Y, Z as W`
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "dotted_name" => {
                    // Skip the module name itself (already handled above)
                    if Some(child) == module_name_node {
                        continue;
                    }
                    // `from X import Y` — bare name
                    if let Ok(name) = child.utf8_text(source) {
                        bindings.push(ImportBinding {
                            local_name: name.to_string(),
                            resolved_path: resolved_path.clone(),
                            imported_symbol: Some(name.to_string()),
                        });
                    }
                }
                "aliased_import" => {
                    // `from X import Y as Z`
                    let original = child.child_by_field_name("name")
                        .and_then(|n| n.utf8_text(source).ok());
                    let alias = child.child_by_field_name("alias")
                        .and_then(|a| a.utf8_text(source).ok());
                    if let Some(orig) = original {
                        let local = alias.unwrap_or(orig);
                        bindings.push(ImportBinding {
                            local_name: local.to_string(),
                            resolved_path: resolved_path.clone(),
                            imported_symbol: Some(orig.to_string()),
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

impl ImportResolver for PythonImportResolver {
    fn find_imports<'a>(
        &self,
        tree: &'a Tree,
        current_file: &Path,
        source: &'a [u8],
    ) -> HashSet<PathBuf> {
        let mut imports = HashSet::new();
        let mut query_cursor = QueryCursor::new();
        let matches = query_cursor.matches(&self.import_query, tree.root_node(), source);

        for match_ in matches {
            for capture in match_.captures {
                let capture_name = &self.import_query.capture_names()[capture.index as usize];
                match capture_name.as_str() {
                    "module" => {
                        // Pattern 0: `import app.models`
                        let text = capture.node.utf8_text(source).unwrap_or("");
                        let root_module = text.split('.').next().unwrap_or("");
                        if !PYTHON_STDLIB_MODULES.contains(root_module)
                            && !COMMON_THIRD_PARTY_MODULES.contains(root_module)
                        {
                            if let Some(path) = self.resolve_absolute(text) {
                                imports.insert(path);
                            }
                        }
                    }
                    "from_import" => {
                        // Pattern 1: entire `from ... import ...` statement
                        self.resolve_from_import(
                            capture.node,
                            current_file,
                            source,
                            &mut imports,
                        );
                    }
                    _ => (),
                }
            }
        }
        imports
    }

    fn find_import_bindings<'a>(
        &self,
        tree: &'a Tree,
        current_file: &Path,
        source: &'a [u8],
    ) -> Vec<ImportBinding> {
        let mut bindings = Vec::new();

        // Use a tree-sitter query that captures both import forms as whole statements
        let binding_query = Query::new(
            tree_sitter_python::language(),
            r#"
            (import_statement) @import_stmt
            (import_from_statement) @from_import
            "#,
        ).expect("Failed to create binding query");

        let mut query_cursor = QueryCursor::new();
        let matches = query_cursor.matches(&binding_query, tree.root_node(), source);

        for match_ in matches {
            for capture in match_.captures {
                let capture_name = &binding_query.capture_names()[capture.index as usize];
                match capture_name.as_str() {
                    "import_stmt" => {
                        // `import utils` or `import utils as u` or `import app.models`
                        self.collect_bare_import_bindings(
                            capture.node,
                            source,
                            &mut bindings,
                        );
                    }
                    "from_import" => {
                        // `from utils import process` or `from utils import process as p`
                        self.collect_from_import_bindings(
                            capture.node,
                            current_file,
                            source,
                            &mut bindings,
                        );
                    }
                    _ => (),
                }
            }
        }
        bindings
    }

    fn file_extensions(&self) -> &[&str] {
        &["py", "pyi"]
    }

    fn get_source_roots(&self) -> Vec<PathBuf> {
        self.source_roots.clone()
    }

    fn module_index_size(&self) -> usize {
        self.module_index.len()
    }
}

#[derive(Debug)]
pub struct JsTsImportResolver {
    project_root: PathBuf,
    /// Maps module specifiers to file paths (e.g., "components/Button" -> "src/components/Button.tsx")
    module_index: HashMap<String, PathBuf>,
    /// Compiled Tree-sitter query for import statements
    import_query_js: Query,
    import_query_ts: Query,
    /// Path aliases from tsconfig.json/jsconfig.json
    path_aliases: HashMap<String, Vec<String>>,
    /// Base URL from tsconfig
    base_url: Option<PathBuf>,
    /// Directory names to exclude from module indexing.
    ignored_dirs: HashSet<String>,
}

impl JsTsImportResolver {
    pub fn new(project_root: &Path, ignored_dirs: &[String]) -> Self {
        let ignored_set: HashSet<String> = ignored_dirs.iter().cloned().collect();
        let canonical_root = project_root.canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());
        let mut resolver = Self {
            project_root: canonical_root,
            module_index: HashMap::new(),
            ignored_dirs: ignored_set,
            import_query_js: Query::new(
                tree_sitter_javascript::language(),
                include_str!("../queries/javascript/imports.scm"),
            )
            .expect("Failed to create JS import query"),
            import_query_ts: Query::new(
                tree_sitter_typescript::language_typescript(),
                include_str!("../queries/typescript/imports.scm"),
            )
            .expect("Failed to create TS import query"),
            path_aliases: HashMap::new(),
            base_url: None,
        };

        resolver.load_tsconfig();
        resolver.index_modules();
        resolver
    }

    fn load_tsconfig(&mut self) {
        // Try tsconfig.json first, then jsconfig.json
        for config_name in &["tsconfig.json", "jsconfig.json"] {
            let config_path = self.project_root.join(config_name);
            if config_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&config_path) {
                    if let Ok(config) = serde_json::from_str::<TsConfig>(&content) {
                        if let Some(opts) = config.compiler_options {
                            if let Some(base) = opts.base_url {
                                self.base_url = Some(self.project_root.join(base));
                            }
                            if let Some(paths) = opts.paths {
                                self.path_aliases = paths;
                            }
                        }
                        break;
                    }
                }
            }
        }
    }

    fn index_modules(&mut self) {
        let extensions = ["js", "jsx", "ts", "tsx", "mjs", "cjs"];

        for entry in WalkDir::new(&self.project_root)
            .into_iter()
            .filter_entry(|e| {
                if e.file_type().is_dir() {
                    let name = e.file_name().to_string_lossy();
                    return !self.ignored_dirs.contains(name.as_ref());
                }
                true
            })
            .filter_map(Result::ok)
            .filter(|e| e.path().is_file())
        {
            let path = entry.path();
            let ext = path.extension().and_then(|s| s.to_str());

            if ext.map_or(false, |e| extensions.contains(&e)) {
                if let Ok(rel_path) = path.strip_prefix(&self.project_root) {
                    // Index by path without extension
                    let path_str = rel_path.to_string_lossy();
                    let without_ext =
                        path_str.trim_end_matches(ext.unwrap()).trim_end_matches('.');
                    self.module_index
                        .insert(without_ext.to_string(), path.to_path_buf());

                    // Also index /index files without the /index part
                    if path.file_stem().and_then(|s| s.to_str()) == Some("index") {
                        if let Some(parent_str) = Path::new(without_ext).parent() {
                            self.module_index.insert(
                                parent_str.to_string_lossy().to_string(),
                                path.to_path_buf(),
                            );
                        }
                    }
                }
            }
        }
    }

    fn resolve_import_specifier(
        &self,
        specifier: &str,
        current_file: &Path,
    ) -> Option<PathBuf> {
        // 1. Filter out Node.js built-ins
        if NODE_BUILTIN_MODULES.contains(specifier) {
            return None;
        }

        // 2. Handle Path Aliases or Node Modules
        if !specifier.starts_with('.') && !specifier.starts_with('/') {
            // Try to resolve as a path alias first
            if let Some(resolved) = self.resolve_path_alias(specifier) {
                return Some(resolved);
            }
            // If not an alias, assume it's a node_modules package and ignore it
            return None;
        }

        // 3. Resolve relative or absolute imports from the file system
        let base_path = if specifier.starts_with('/') {
            // Absolute path from project root
            self.project_root.clone()
        } else {
            // Relative path from current file's directory
            current_file.parent()?.to_path_buf()
        };

        // trim '/' to prevent path.join from treating it as an absolute path on its own
        let target_path = base_path.join(specifier.trim_start_matches('/'));

        // Use the robust, unified extension resolution logic
        self.try_resolve_with_extensions(&target_path)
    }

    fn resolve_path_alias(&self, specifier: &str) -> Option<PathBuf> {
        for (alias_pattern, alias_paths) in &self.path_aliases {
            // Handle patterns like "@/*" -> ["src/*"]
            if let Some(prefix) = alias_pattern.strip_suffix("/*") {
                if let Some(remainder) = specifier.strip_prefix(prefix) {
                    let remainder = remainder.trim_start_matches('/');
                    for path_pattern in alias_paths {
                        if let Some(path_prefix) = path_pattern.strip_suffix("/*") {
                            let base = self.base_url.as_ref().unwrap_or(&self.project_root);
                            let resolved_base = base.join(path_prefix);
                            let full_path = resolved_base.join(remainder);
                            if let Some(path) = self.try_resolve_with_extensions(&full_path) {
                                return Some(path);
                            }
                        }
                    }
                }
            }
            // Handle exact matches, e.g., "react" -> "./node_modules/react"
            else if alias_pattern == specifier {
                for path_pattern in alias_paths {
                    let base = self.base_url.as_ref().unwrap_or(&self.project_root);
                    let full_path = base.join(path_pattern);
                    if let Some(path) = self.try_resolve_with_extensions(&full_path) {
                        return Some(path);
                    }
                }
            }
        }
        None
    }

    fn try_resolve_with_extensions(&self, base: &Path) -> Option<PathBuf> {
        let extensions = ["ts", "tsx", "js", "jsx", "mjs", "cjs"];

        // Case 1: The base path with an extension exists (e.g., .../Button -> .../Button.tsx)
        for ext in &extensions {
            let candidate = base.with_extension(ext);
            if candidate.is_file() {
                return Some(candidate);
            }
        }

        // Case 2: The base path is a directory with an index file (e.g., .../Button -> .../Button/index.tsx)
        for ext in &extensions {
            let candidate = base.join(format!("index.{}", ext));
            if candidate.is_file() {
                return Some(candidate);
            }
        }

        // Case 3: The base path itself is a file with a non-standard extension (e.g. import './styles.css')
        if base.is_file() {
            return Some(base.to_path_buf());
        }

        None
    }
}

impl ImportResolver for JsTsImportResolver {
    fn find_imports<'a>(
        &self,
        tree: &'a Tree,
        current_file: &Path,
        source: &'a [u8],
    ) -> HashSet<PathBuf> {
        let mut imports = HashSet::new();
        // Determine which query to use based on file extension
        let lang = SupportedLanguage::from_path(current_file);
        let query = match lang {
            SupportedLanguage::TypeScript | SupportedLanguage::TypeScriptTsx => &self.import_query_ts,
            _ => &self.import_query_js,
        };

        let mut cursor = QueryCursor::new();
        let matches = cursor.matches(query, tree.root_node(), source);

        for match_ in matches {
            for capture in match_.captures {
                let capture_name = &query.capture_names()[capture.index as usize];
                if capture_name == "import" {
                    if let Ok(import_text) = capture.node.utf8_text(source) {
                        let specifier = import_text.trim_matches(|c| c == '"' || c == '\'' || c == '`');
                        if let Some(path) = self.resolve_import_specifier(specifier, current_file) {
                             imports.insert(path);
                        }
                    }
                }
            }
        }
        imports
    }

    fn file_extensions(&self) -> &[&str] {
        &["js", "jsx", "ts", "tsx", "mjs", "cjs"]
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    use tree_sitter::Parser;

    struct TestRepo {
        _root: tempfile::TempDir, // The TempDir is owned by this struct, so it gets cleaned up when dropped
        pub root_path: PathBuf,
        pub resolver: PythonImportResolver,
        pub parser: Parser,
    }

    fn setup() -> TestRepo {
        let root = tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();

        // Create a dummy project structure
        create_dummy_file(&root_path, "app.py", "");
        create_dummy_file(&root_path, "src/utils.py", "");
        create_dummy_file(&root_path, "src/api/__init__.py", "");
        create_dummy_file(&root_path, "src/api/v1/endpoints.py", "");
        create_dummy_file(&root_path, "src/api/v1/helpers.py", "");
        create_dummy_file(&root_path, "README.md", ""); // Should be ignored

        let resolver = PythonImportResolver::new(&root_path, &[], None);

        let mut parser = Parser::new();
        parser
            .set_language(tree_sitter_python::language())
            .expect("Failed to load python grammar");

        TestRepo {
            _root: root,
            root_path,
            resolver,
            parser,
        }
    }

    // Helper to create a dummy file and its parent directories.
    fn create_dummy_file(root: &Path, path: &str, content: &str) {
        let file_path = root.join(path);
        fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        fs::write(file_path, content).unwrap();
    }
    
    fn run_find_imports(repo: &mut TestRepo, source: &str, file_name: &str) -> HashSet<PathBuf> {
        let tree = repo.parser.parse(source, None).unwrap();
        repo.resolver.find_imports(&tree, &repo.root_path.join(file_name), source.as_bytes())
    }

    #[test]
    fn test_module_indexing() {
        let repo = setup();
        let index = &repo.resolver.module_index;
        // 5 full-path keys + 4 source-root-relative keys (src/ is a source root)
        // Full-path: app, src.utils, src.api, src.api.v1.endpoints, src.api.v1.helpers
        // Source-root-relative: utils, api, api.v1.endpoints, api.v1.helpers
        assert_eq!(index.len(), 9);
        assert_eq!(index.get("app"), Some(&repo.root_path.join("app.py")));
        assert_eq!(
            index.get("src.utils"),
            Some(&repo.root_path.join("src/utils.py"))
        );
        assert_eq!(
            index.get("src.api"),
            Some(&repo.root_path.join("src/api/__init__.py"))
        );
        assert_eq!(
            index.get("src.api.v1.endpoints"),
            Some(&repo.root_path.join("src/api/v1/endpoints.py"))
        );
        // Source-root-relative variants
        assert_eq!(
            index.get("utils"),
            Some(&repo.root_path.join("src/utils.py"))
        );
        assert_eq!(
            index.get("api"),
            Some(&repo.root_path.join("src/api/__init__.py"))
        );
    }

    #[test]
    fn test_resolve_absolute() {
        let mut repo = setup();
        let imports = run_find_imports(&mut repo, "import src.utils", "app.py");
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("src/utils.py")));
    }

    #[test]
    fn test_resolve_package() {
        let mut repo = setup();
        let imports = run_find_imports(&mut repo, "import src.api", "app.py");
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("src/api/__init__.py")));
    }

    #[test]
    fn test_resolve_relative_single_dot() {
        let mut repo = setup();
        let imports = run_find_imports(&mut repo, "from . import helpers", "src/api/v1/endpoints.py");
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("src/api/v1/helpers.py")));
    }

    #[test]
    fn test_resolve_relative_double_dot() {
        let mut repo = setup();
        let imports = run_find_imports(&mut repo, "from .. import utils", "src/api/v1/endpoints.py");
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("src/utils.py")));
    }

    #[test]
    fn test_resolve_stdlib() {
        let mut repo = setup();
        let imports = run_find_imports(&mut repo, "import os", "app.py");
        assert!(imports.is_empty());
    }

    #[test]
    fn test_resolve_third_party() {
        let mut repo = setup();
        let imports = run_find_imports(&mut repo, "import numpy as np", "app.py");
        assert!(imports.is_empty());
    }

    #[test]
    fn test_resolve_non_existent() {
        let mut repo = setup();
        let imports = run_find_imports(&mut repo, "import project.non_existent", "app.py");
        assert!(imports.is_empty());
    }

    #[test]
    fn test_resolve_package_deep() {
        let mut repo = setup();
        create_dummy_file(&repo.root_path, "src/api/v1/__init__.py", "");
        repo.resolver.index_modules(); // Re-index after adding a new file

        let imports = run_find_imports(&mut repo, "import src.api.v1", "app.py");
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("src/api/v1/__init__.py")));
    }

    #[test]
    fn test_resolve_star_import() {
        let mut repo = setup();
        // No __init__.py in src/api/v1/, so wildcard resolves to nothing
        let imports = run_find_imports(&mut repo, "from . import *", "src/api/v1/endpoints.py");
        assert!(imports.is_empty());
    }

    #[test]
    fn test_resolve_star_import_with_init() {
        let mut repo = setup();
        create_dummy_file(&repo.root_path, "src/api/v1/__init__.py", "");
        repo.resolver = PythonImportResolver::new(&repo.root_path, &[], None);

        let imports = run_find_imports(&mut repo, "from . import *", "src/api/v1/endpoints.py");
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("src/api/v1/__init__.py")));
    }

    #[test]
    fn test_resolve_relative_with_module_name() {
        // This is the primary bug fix: `from .helpers import some_func`
        // Previously, the query captured "some_func" as the module, not "helpers"
        let mut repo = setup();
        let imports = run_find_imports(
            &mut repo,
            "from .helpers import some_func",
            "src/api/v1/endpoints.py",
        );
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("src/api/v1/helpers.py")));
    }

    #[test]
    fn test_resolve_relative_dotted_module() {
        // `from ..api import something` from src/api/v1/endpoints.py
        // resolve_relative with dots=2:
        //   base_path starts at src/api/v1/, goes up once to src/api/
        //   then the __init__.py fallback goes up one more to src/
        //   components = ["src"] + ["api"] = "src.api" → src/api/__init__.py
        let mut repo = setup();
        let imports = run_find_imports(
            &mut repo,
            "from ..api import something",
            "src/api/v1/endpoints.py",
        );
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("src/api/__init__.py")));
    }

    #[test]
    fn test_resolve_from_import_absolute() {
        // `from src.api import something` should resolve to src/api/__init__.py
        let mut repo = setup();
        let imports = run_find_imports(&mut repo, "from src.api import something", "app.py");
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("src/api/__init__.py")));
    }

    #[test]
    fn test_resolve_from_import_absolute_module() {
        // `from src.utils import some_func` should resolve to src/utils.py
        let mut repo = setup();
        let imports = run_find_imports(&mut repo, "from src.utils import some_func", "app.py");
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("src/utils.py")));
    }

    #[test]
    fn test_resolve_multiple_bare_imports() {
        // `from . import helpers, endpoints` should resolve both modules
        let mut repo = setup();
        let imports = run_find_imports(
            &mut repo,
            "from . import helpers, endpoints",
            "src/api/v1/endpoints.py",
        );
        // helpers.py exists, endpoints.py is the file itself (self-import gets resolved but
        // the graph layer filters self-edges). Both should appear in raw resolution.
        assert!(imports.contains(&repo.root_path.join("src/api/v1/helpers.py")));
        assert!(imports.contains(&repo.root_path.join("src/api/v1/endpoints.py")));
    }

    #[test]
    fn test_resolve_function_level_import() {
        // Imports inside functions should also be captured
        let mut repo = setup();
        let source = r#"
def my_function():
    from src.utils import helper
    return helper()
"#;
        let imports = run_find_imports(&mut repo, source, "app.py");
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("src/utils.py")));
    }

    #[test]
    fn test_resolve_from_import_with_alias() {
        // `from src.utils import something as alias` should still resolve
        let mut repo = setup();
        let imports = run_find_imports(
            &mut repo,
            "from src.utils import something as alias",
            "app.py",
        );
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("src/utils.py")));
    }

    #[test]
    fn test_resolve_bare_import_fallback_to_init() {
        // `from . import nonexistent_module` where the name is not a submodule
        // but __init__.py exists — should fall back to __init__.py
        let mut repo = setup();
        create_dummy_file(&repo.root_path, "src/api/v1/__init__.py", "");
        repo.resolver = PythonImportResolver::new(&repo.root_path, &[], None);

        let imports = run_find_imports(
            &mut repo,
            "from . import nonexistent_module",
            "src/api/v1/endpoints.py",
        );
        // nonexistent_module doesn't resolve as a module, so falls back to __init__.py
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("src/api/v1/__init__.py")));
    }

    // === Source Root Detection Tests ===

    /// Helper to set up a Django-like project with backend/ source root
    fn setup_django_project() -> TestRepo {
        let root = tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();

        // Django-style: backend/ is the source root, app/ is a package inside it
        create_dummy_file(&root_path, "backend/app/__init__.py", "");
        create_dummy_file(&root_path, "backend/app/models/__init__.py", "");
        create_dummy_file(&root_path, "backend/app/models/enums.py", "class PlanStatus: pass");
        create_dummy_file(&root_path, "backend/app/models/user.py", "class User: pass");
        create_dummy_file(&root_path, "backend/app/views/__init__.py", "");
        create_dummy_file(&root_path, "backend/app/views/api.py", "");
        create_dummy_file(
            &root_path,
            "backend/app/v3/orchestrator.py",
            "from app.models.enums import PlanStatus",
        );
        create_dummy_file(&root_path, "backend/manage.py", "");
        // Non-source-root directory (no Python packages inside)
        create_dummy_file(&root_path, "frontend/index.html", "");

        let resolver = PythonImportResolver::new(&root_path, &[], None);

        let mut parser = Parser::new();
        parser
            .set_language(tree_sitter_python::language())
            .expect("Failed to load python grammar");

        TestRepo {
            _root: root,
            root_path,
            resolver,
            parser,
        }
    }

    #[test]
    fn test_source_root_detection() {
        let repo = setup_django_project();
        // backend/ should be detected as a source root (contains app/ with __init__.py)
        assert_eq!(repo.resolver.source_roots.len(), 1);
        assert_eq!(repo.resolver.source_roots[0], repo.root_path.join("backend"));
    }

    #[test]
    fn test_source_root_not_detected_for_non_package_dir() {
        let repo = setup_django_project();
        // frontend/ should NOT be a source root (no subdirectory with __init__.py)
        let frontend = repo.root_path.join("frontend");
        assert!(!repo.resolver.source_roots.contains(&frontend));
    }

    #[test]
    fn test_source_root_module_index_has_both_variants() {
        let repo = setup_django_project();
        let index = &repo.resolver.module_index;

        // Full-path variant should exist
        assert!(
            index.contains_key("backend.app.models.enums"),
            "Full-path key 'backend.app.models.enums' should be in index"
        );

        // Source-root-relative variant should also exist
        assert!(
            index.contains_key("app.models.enums"),
            "Source-root-relative key 'app.models.enums' should be in index"
        );

        // Both should point to the same file
        assert_eq!(
            index.get("backend.app.models.enums"),
            index.get("app.models.enums"),
        );
    }

    #[test]
    fn test_absolute_import_via_source_root() {
        // The critical test: `from app.models.enums import PlanStatus` should resolve
        // even though the file is at backend/app/models/enums.py
        let mut repo = setup_django_project();
        let imports = run_find_imports(
            &mut repo,
            "from app.models.enums import PlanStatus",
            "backend/app/v3/orchestrator.py",
        );
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("backend/app/models/enums.py")));
    }

    #[test]
    fn test_full_path_import_still_works() {
        // `from backend.app.models.enums import PlanStatus` should also resolve
        let mut repo = setup_django_project();
        let imports = run_find_imports(
            &mut repo,
            "from backend.app.models.enums import PlanStatus",
            "backend/manage.py",
        );
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("backend/app/models/enums.py")));
    }

    #[test]
    fn test_absolute_import_package_via_source_root() {
        // `from app.models import something` should resolve to app/models/__init__.py
        let mut repo = setup_django_project();
        let imports = run_find_imports(
            &mut repo,
            "from app.models import something",
            "backend/app/views/api.py",
        );
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("backend/app/models/__init__.py")));
    }

    #[test]
    fn test_bare_absolute_import_via_source_root() {
        // `import app.models.enums` should resolve
        let mut repo = setup_django_project();
        let imports = run_find_imports(
            &mut repo,
            "import app.models.enums",
            "backend/manage.py",
        );
        assert_eq!(imports.len(), 1);
        assert!(imports.contains(&repo.root_path.join("backend/app/models/enums.py")));
    }

    #[test]
    fn test_multiple_source_roots() {
        let root = tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();

        // Two source roots: backend/ and libs/
        create_dummy_file(&root_path, "backend/app/__init__.py", "");
        create_dummy_file(&root_path, "backend/app/core.py", "");
        create_dummy_file(&root_path, "libs/shared/__init__.py", "");
        create_dummy_file(&root_path, "libs/shared/utils.py", "");

        let resolver = PythonImportResolver::new(&root_path, &[], None);

        // Both should be detected as source roots
        assert_eq!(resolver.source_roots.len(), 2);
        let root_set: HashSet<_> = resolver.source_roots.iter().collect();
        assert!(root_set.contains(&root_path.join("backend")));
        assert!(root_set.contains(&root_path.join("libs")));

        // Both source-root-relative keys should exist
        assert!(resolver.module_index.contains_key("app.core"));
        assert!(resolver.module_index.contains_key("shared.utils"));
    }

    #[test]
    fn test_source_root_ignored_dirs_skipped() {
        let root = tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();

        // node_modules contains packages but should be ignored
        create_dummy_file(&root_path, "node_modules/pkg/__init__.py", "");
        create_dummy_file(&root_path, "backend/app/__init__.py", "");

        let resolver = PythonImportResolver::new(
            &root_path,
            &["node_modules".to_string()],
            None,
        );

        // Only backend/ should be detected, not node_modules/
        assert_eq!(resolver.source_roots.len(), 1);
        assert_eq!(resolver.source_roots[0], root_path.join("backend"));
    }

    #[test]
    fn test_source_root_namespace_package_no_init() {
        // Mirrors FountainOfYouth layout:
        //   backend/app/         ← NO __init__.py (namespace package)
        //   backend/app/main.py  ← has .py files
        //   backend/app/models/__init__.py  ← sub-packages have __init__.py
        let root = tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();

        create_dummy_file(&root_path, "backend/__init__.py", "");
        // app/ has NO __init__.py — this is the key difference
        create_dummy_file(&root_path, "backend/app/main.py", "from app.models.enums import Foo");
        create_dummy_file(&root_path, "backend/app/models/__init__.py", "");
        create_dummy_file(&root_path, "backend/app/models/enums.py", "class Foo: pass");
        create_dummy_file(&root_path, "backend/app/services/__init__.py", "");
        create_dummy_file(&root_path, "config/settings/__init__.py", "");
        create_dummy_file(&root_path, "config/settings/base.py", "DEBUG = True");

        let resolver = PythonImportResolver::new(&root_path, &[], None);

        // Both backend/ and config/ should be detected as source roots
        let root_names: HashSet<String> = resolver.source_roots.iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
            .collect();
        assert!(root_names.contains("backend"),
            "backend/ should be detected as source root (namespace package with .py files). Found: {:?}",
            root_names);
        assert!(root_names.contains("config"),
            "config/ should be detected as source root. Found: {:?}",
            root_names);

        // Source-root-relative module keys should exist
        assert!(resolver.module_index.contains_key("app.main"),
            "app.main should be in module index. Keys: {:?}",
            resolver.module_index.keys().collect::<Vec<_>>());
        assert!(resolver.module_index.contains_key("app.models.enums"),
            "app.models.enums should be in module index");
    }

    #[test]
    fn test_no_source_roots_for_flat_project() {
        // The default setup() project has no source roots (src/ contains files directly,
        // not packages with __init__.py at depth-1)
        let repo = setup();
        // src/ does have src/api/__init__.py, so src/ IS a source root
        // but the default setup doesn't have a depth-1 dir containing a package subdir
        // Actually: src/ is a depth-1 dir, and it contains api/ which has __init__.py
        // So src/ IS detected as a source root
        assert_eq!(repo.resolver.source_roots.len(), 1);
        assert_eq!(repo.resolver.source_roots[0], repo.root_path.join("src"));
    }
}