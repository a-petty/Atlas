use lazy_static::lazy_static;
use log::{debug, info};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
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

// ---------------------------------------------------------------------------
// package.json parsing struct
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct PackageJson {
    dependencies: Option<HashMap<String, serde_json::Value>>,
    #[serde(rename = "devDependencies")]
    dev_dependencies: Option<HashMap<String, serde_json::Value>>,
    #[serde(rename = "peerDependencies")]
    peer_dependencies: Option<HashMap<String, serde_json::Value>>,
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

    /// Get unique top-level module names visible in the module index. Default: empty.
    fn get_known_root_modules(&self) -> Vec<String> { Vec::new() }

    /// Get the number of import resolution attempts. Default: 0.
    fn get_attempted_imports(&self) -> usize { 0 }

    /// Get the number of failed import resolutions. Default: 0.
    fn get_failed_imports(&self) -> usize { 0 }

    /// Get the number of detected third-party packages. Default: 0.
    fn get_third_party_count(&self) -> usize { 0 }

    /// Diagnostic: look up a module path in the index and return what it resolves to.
    fn debug_module_lookup(&self, _module_path: &str) -> Option<PathBuf> { None }

    /// Diagnostic: trace resolution of a from-import statement from a given file.
    /// Returns a list of (step_description, result) pairs showing the resolution process.
    fn debug_resolve_import(&self, _import_source: &str, _current_file: &Path) -> Vec<(String, String)> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// pyproject.toml parsing structs
// ---------------------------------------------------------------------------

/// Top-level pyproject.toml structure.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct PyProjectToml {
    tool: Option<ToolSection>,
    project: Option<ProjectSection>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ProjectSection {
    name: Option<String>,
    dependencies: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct ToolSection {
    hatch: Option<HatchConfig>,
    setuptools: Option<SetuptoolsConfig>,
    poetry: Option<PoetryConfig>,
    uv: Option<UvConfig>,
}

// --- Hatch ---
#[derive(Debug, Deserialize)]
struct HatchConfig {
    build: Option<HatchBuildConfig>,
}

#[derive(Debug, Deserialize)]
struct HatchBuildConfig {
    targets: Option<HatchTargets>,
}

#[derive(Debug, Deserialize)]
struct HatchTargets {
    wheel: Option<HatchWheel>,
}

#[derive(Debug, Deserialize)]
struct HatchWheel {
    packages: Option<Vec<String>>,
}

// --- Setuptools ---
#[derive(Debug, Deserialize)]
struct SetuptoolsConfig {
    #[serde(rename = "package-dir")]
    package_dir: Option<HashMap<String, String>>,
    packages: Option<SetuptoolsPackages>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
#[allow(dead_code)]
enum SetuptoolsPackages {
    List(Vec<String>),
    Find(SetuptoolsFind),
}

#[derive(Debug, Deserialize)]
struct SetuptoolsFind {
    #[serde(rename = "where")]
    where_dirs: Option<Vec<String>>,
}

// --- Poetry ---
#[derive(Debug, Deserialize)]
struct PoetryConfig {
    packages: Option<Vec<PoetryPackage>>,
    dependencies: Option<toml::value::Table>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct PoetryPackage {
    include: Option<String>,
    from: Option<String>,
}

// --- UV ---
#[derive(Debug, Deserialize)]
struct UvConfig {
    workspace: Option<UvWorkspace>,
}

#[derive(Debug, Deserialize)]
struct UvWorkspace {
    members: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// pyproject.toml helper functions
// ---------------------------------------------------------------------------

/// Parse a pyproject.toml file, returning None on any failure.
fn parse_pyproject_toml(path: &Path) -> Option<PyProjectToml> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            debug!("Cannot read {}: {}", path.display(), e);
            return None;
        }
    };
    match toml::from_str::<PyProjectToml>(&content) {
        Ok(parsed) => Some(parsed),
        Err(e) => {
            debug!("Cannot parse {}: {}", path.display(), e);
            None
        }
    }
}

/// Expand workspace member glob patterns (e.g., "providers/*") to directories
/// that contain a pyproject.toml.
fn expand_workspace_members(project_root: &Path, patterns: &[String]) -> Vec<PathBuf> {
    let mut members = Vec::new();
    for pattern in patterns {
        let full_pattern = project_root.join(pattern).to_string_lossy().to_string();
        match glob::glob(&full_pattern) {
            Ok(paths) => {
                for entry in paths.filter_map(Result::ok) {
                    if entry.is_dir() && entry.join("pyproject.toml").exists() {
                        let canonical = entry.canonicalize().unwrap_or(entry);
                        members.push(canonical);
                    }
                }
            }
            Err(e) => {
                debug!("Invalid glob pattern '{}': {}", pattern, e);
            }
        }
    }
    members
}

/// Given a package path like "src/airflow", find the source root directory.
/// The source root is the directory that should be on sys.path — i.e., the parent
/// of the first component that is an actual Python package.
///
/// Examples:
///   "src/airflow" with src/airflow/__init__.py → returns src/
///   "airflow" with airflow/__init__.py → returns base_dir
///   "src/airflow" with no __init__.py anywhere → returns None
fn derive_source_dir_from_package_path(base_dir: &Path, pkg_path: &str) -> Option<PathBuf> {
    let components: Vec<&str> = pkg_path.split('/').filter(|s| !s.is_empty()).collect();
    if components.is_empty() {
        return None;
    }

    // Walk the components from the start. The first component that is a Python package
    // (has __init__.py) tells us the source root is its parent.
    let mut current = base_dir.to_path_buf();
    for (i, component) in components.iter().enumerate() {
        current = current.join(component);
        if current.join("__init__.py").exists() {
            // This component is a package — source root is everything before it
            let mut source_root = base_dir.to_path_buf();
            for c in &components[..i] {
                source_root = source_root.join(c);
            }
            return Some(source_root.canonicalize().unwrap_or(source_root));
        }
    }

    // No __init__.py found anywhere — check if the full path has .py files (namespace package)
    let full_path = base_dir.join(pkg_path);
    if full_path.is_dir() {
        let has_py = std::fs::read_dir(&full_path)
            .map(|entries| entries.filter_map(Result::ok).any(|e| {
                e.path().extension().and_then(|ext| ext.to_str()) == Some("py")
            }))
            .unwrap_or(false);
        if has_py {
            // Namespace package — source root is its parent
            let mut source_root = base_dir.to_path_buf();
            for c in &components[..components.len() - 1] {
                source_root = source_root.join(c);
            }
            return Some(source_root.canonicalize().unwrap_or(source_root));
        }
    }

    None
}

/// Extract source root from a single pyproject.toml file.
/// Tries: Hatch packages → Setuptools package-dir/find.where → Poetry packages.from
fn extract_source_root(member_dir: &Path) -> Option<PathBuf> {
    let pyproject_path = member_dir.join("pyproject.toml");
    let pyproject = parse_pyproject_toml(&pyproject_path)?;
    let tool = pyproject.tool?;

    // Priority 1: Hatch packages
    if let Some(hatch) = &tool.hatch {
        if let Some(build) = &hatch.build {
            if let Some(targets) = &build.targets {
                if let Some(wheel) = &targets.wheel {
                    if let Some(packages) = &wheel.packages {
                        for pkg_path in packages {
                            if let Some(root) = derive_source_dir_from_package_path(member_dir, pkg_path) {
                                info!("pyproject.toml (Hatch): packages='{}' → source root: {}",
                                      pkg_path, root.display());
                                return Some(root);
                            }
                        }
                    }
                }
            }
        }
    }

    // Priority 2: Setuptools package-dir or find.where
    if let Some(setuptools) = &tool.setuptools {
        // [tool.setuptools.package-dir] "" = "src"
        if let Some(package_dir) = &setuptools.package_dir {
            if let Some(src_dir) = package_dir.get("") {
                let root = member_dir.join(src_dir);
                if root.is_dir() {
                    let canonical = root.canonicalize().unwrap_or(root);
                    info!("pyproject.toml (Setuptools package-dir): '' = '{}' → source root: {}",
                          src_dir, canonical.display());
                    return Some(canonical);
                }
            }
        }
        // [tool.setuptools.packages.find] where = ["src"]
        if let Some(SetuptoolsPackages::Find(find)) = &setuptools.packages {
            if let Some(where_dirs) = &find.where_dirs {
                for dir in where_dirs {
                    let root = member_dir.join(dir);
                    if root.is_dir() {
                        let canonical = root.canonicalize().unwrap_or(root);
                        info!("pyproject.toml (Setuptools find.where): '{}' → source root: {}",
                              dir, canonical.display());
                        return Some(canonical);
                    }
                }
            }
        }
    }

    // Priority 3: Poetry packages.from
    if let Some(poetry) = &tool.poetry {
        if let Some(packages) = &poetry.packages {
            for pkg in packages {
                if let Some(from_dir) = &pkg.from {
                    let root = member_dir.join(from_dir);
                    if root.is_dir() {
                        let canonical = root.canonicalize().unwrap_or(root);
                        info!("pyproject.toml (Poetry packages.from): '{}' → source root: {}",
                              from_dir, canonical.display());
                        return Some(canonical);
                    }
                }
            }
        }
    }

    None
}

/// Resolves Python import statements to absolute file paths.
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
    /// Number of import resolution attempts (atomic for thread-safe counting).
    attempted_imports: AtomicUsize,
    /// Number of failed import resolutions (atomic for thread-safe counting).
    failed_imports: AtomicUsize,
    /// Third-party package root module names (auto-detected + hardcoded fallback).
    third_party_packages: HashSet<String>,
}

impl std::fmt::Debug for PythonImportResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PythonImportResolver")
            .field("project_root", &self.project_root)
            .field("module_index_size", &self.module_index.len())
            .field("source_roots", &self.source_roots)
            .field("attempted_imports", &self.attempted_imports.load(Ordering::Relaxed))
            .field("failed_imports", &self.failed_imports.load(Ordering::Relaxed))
            .field("third_party_packages_count", &self.third_party_packages.len())
            .finish()
    }
}

impl PythonImportResolver {
    /// Creates a new `PythonImportResolver` and indexes all Python modules in the project.
    /// `ignored_dirs` contains directory names (not paths) to exclude from module indexing.
    /// `explicit_source_roots` optionally overrides auto-detection of source roots.
    pub fn new(project_root: &Path, ignored_dirs: &[String], explicit_source_roots: Option<&[String]>) -> Self {
        let ignored_set: HashSet<String> = ignored_dirs.iter().cloned().collect();
        let canonical_root = project_root.canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());
        let third_party_packages = Self::discover_third_party_packages(&canonical_root);
        let mut resolver = Self {
            project_root: canonical_root.clone(),
            module_index: HashMap::new(),
            ignored_dirs: ignored_set,
            source_roots: Vec::new(),
            attempted_imports: AtomicUsize::new(0),
            failed_imports: AtomicUsize::new(0),
            third_party_packages,
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

    // -----------------------------------------------------------------------
    // Third-party package discovery
    // -----------------------------------------------------------------------

    /// Known pip-name → import-name mismatches.
    fn known_package_mappings() -> HashMap<&'static str, &'static str> {
        [
            ("python-jose", "jose"),
            ("python-dateutil", "dateutil"),
            ("python-multipart", "multipart"),
            ("python-dotenv", "dotenv"),
            ("pillow", "PIL"),
            ("scikit-learn", "sklearn"),
            ("beautifulsoup4", "bs4"),
            ("pyyaml", "yaml"),
            ("pymongo", "pymongo"),
            ("psycopg2-binary", "psycopg2"),
            ("opencv-python", "cv2"),
            ("opencv-python-headless", "cv2"),
        ].iter().cloned().collect()
    }

    /// Normalize a pip package name to likely import name(s).
    fn normalize_package_name(raw: &str) -> Vec<String> {
        // Strip version specifiers, extras, inline comments
        let name = raw.split(|c: char| c == '=' || c == '>' || c == '<' || c == '~' || c == '!' || c == ';')
            .next()
            .unwrap_or(raw)
            .trim();
        // Strip extras like [standard]
        let name = name.split('[').next().unwrap_or(name).trim();
        if name.is_empty() {
            return Vec::new();
        }

        let lower = name.to_lowercase();
        let mappings = Self::known_package_mappings();

        let mut results = Vec::new();
        if let Some(&mapped) = mappings.get(lower.as_str()) {
            results.push(mapped.to_string());
        }

        // Standard normalization: hyphens → underscores, lowercase
        let normalized = lower.replace('-', "_");
        if !results.contains(&normalized) {
            results.push(normalized.clone());
        }

        // Also add root prefix (first segment before - or _) as secondary match
        let root = lower.split(|c: char| c == '-' || c == '_').next().unwrap_or(&lower);
        let root = root.to_string();
        if root != normalized && !results.contains(&root) {
            results.push(root);
        }

        results
    }

    /// Parse a requirements.txt file, returning normalized import names.
    fn parse_requirements_txt(path: &Path) -> HashSet<String> {
        let mut packages = HashSet::new();
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return packages,
        };
        for line in content.lines() {
            let line = line.trim();
            // Strip inline comments
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            // Skip flags: -r, --requirement, -e, -f, --index-url, etc.
            if line.starts_with('-') {
                continue;
            }
            for name in Self::normalize_package_name(line) {
                packages.insert(name);
            }
        }
        packages
    }

    /// Extract dependency names from a parsed pyproject.toml.
    fn extract_dependencies_from_pyproject(pyproject: &PyProjectToml) -> HashSet<String> {
        let mut packages = HashSet::new();

        // [project.dependencies]
        if let Some(project) = &pyproject.project {
            if let Some(deps) = &project.dependencies {
                for dep in deps {
                    for name in Self::normalize_package_name(dep) {
                        packages.insert(name);
                    }
                }
            }
        }

        // [tool.poetry.dependencies]
        if let Some(tool) = &pyproject.tool {
            if let Some(poetry) = &tool.poetry {
                if let Some(deps) = &poetry.dependencies {
                    for key in deps.keys() {
                        if key == "python" {
                            continue;
                        }
                        for name in Self::normalize_package_name(key) {
                            packages.insert(name);
                        }
                    }
                }
            }
        }

        packages
    }

    /// Discover third-party packages from requirements.txt and pyproject.toml files.
    /// Merges with the hardcoded COMMON_THIRD_PARTY_MODULES fallback.
    fn discover_third_party_packages(project_root: &Path) -> HashSet<String> {
        let mut packages: HashSet<String> = COMMON_THIRD_PARTY_MODULES.iter()
            .map(|s| s.to_string())
            .collect();

        let mut discovered = 0usize;

        // requirements.txt in project root
        let req_path = project_root.join("requirements.txt");
        if req_path.is_file() {
            let found = Self::parse_requirements_txt(&req_path);
            discovered += found.len();
            debug!("Found {} packages in {}", found.len(), req_path.display());
            packages.extend(found);
        }

        // requirements/*.txt (all files in requirements/ dir)
        let req_dir = project_root.join("requirements");
        if req_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&req_dir) {
                for entry in entries.filter_map(Result::ok) {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("txt") {
                        let found = Self::parse_requirements_txt(&p);
                        discovered += found.len();
                        debug!("Found {} packages in {}", found.len(), p.display());
                        packages.extend(found);
                    }
                }
            }
        }

        // One level deep: */requirements.txt
        if let Ok(entries) = std::fs::read_dir(project_root) {
            for entry in entries.filter_map(Result::ok) {
                let p = entry.path();
                if p.is_dir() {
                    let sub_req = p.join("requirements.txt");
                    if sub_req.is_file() {
                        let found = Self::parse_requirements_txt(&sub_req);
                        discovered += found.len();
                        debug!("Found {} packages in {}", found.len(), sub_req.display());
                        packages.extend(found);
                    }
                }
            }
        }

        // pyproject.toml in project root
        let pyproject_path = project_root.join("pyproject.toml");
        if pyproject_path.is_file() {
            if let Some(pyproject) = parse_pyproject_toml(&pyproject_path) {
                let found = Self::extract_dependencies_from_pyproject(&pyproject);
                discovered += found.len();
                debug!("Found {} packages in {}", found.len(), pyproject_path.display());
                packages.extend(found);
            }
        }

        // One level deep: */pyproject.toml
        if let Ok(entries) = std::fs::read_dir(project_root) {
            for entry in entries.filter_map(Result::ok) {
                let p = entry.path();
                if p.is_dir() {
                    let sub_pyproject = p.join("pyproject.toml");
                    if sub_pyproject.is_file() {
                        if let Some(pyproject) = parse_pyproject_toml(&sub_pyproject) {
                            let found = Self::extract_dependencies_from_pyproject(&pyproject);
                            discovered += found.len();
                            debug!("Found {} packages in {}", found.len(), sub_pyproject.display());
                            packages.extend(found);
                        }
                    }
                }
            }
        }

        if discovered > 0 {
            info!("Third-party packages: {} total ({} from dependency files, {} hardcoded)",
                  packages.len(), discovered, COMMON_THIRD_PARTY_MODULES.len());
        }

        packages
    }

    /// Main source root detection pipeline.
    /// Priority: pyproject.toml → src/ heuristic → legacy depth-1 scan.
    fn detect_source_roots(&self) -> Vec<PathBuf> {
        // Priority 1: pyproject.toml-driven discovery
        let pyproject_roots = self.discover_source_roots_from_pyproject();
        if !pyproject_roots.is_empty() {
            return pyproject_roots;
        }

        // Priority 2: src/ layout heuristic
        let src_roots = self.detect_src_layout_heuristic();
        if !src_roots.is_empty() {
            return src_roots;
        }

        // Priority 3: Legacy depth-1 scan
        self.detect_source_roots_legacy()
    }

    /// Detect source roots for src/ layout projects.
    /// Checks for src/ (or lib/, source/) directories that are NOT Python packages
    /// themselves (no __init__.py) but contain Python packages or modules.
    fn detect_src_layout_heuristic(&self) -> Vec<PathBuf> {
        let candidate_names = ["src", "lib", "source"];
        let mut roots = Vec::new();

        for name in &candidate_names {
            let candidate = self.project_root.join(name);
            if !candidate.is_dir() {
                continue;
            }

            // Guard: if src/__init__.py exists, it's a package, not a source root
            if candidate.join("__init__.py").exists() {
                debug!("src/ heuristic: {}/{} has __init__.py, skipping (it's a package)",
                       self.project_root.display(), name);
                continue;
            }

            // Check: src/ must contain at least one Python package or module
            let has_python_content = if let Ok(entries) = std::fs::read_dir(&candidate) {
                entries.filter_map(Result::ok).any(|entry| {
                    let path = entry.path();
                    if path.is_dir() {
                        let dir_name = path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("");
                        if dir_name.starts_with('.') || dir_name == "__pycache__" {
                            return false;
                        }
                        // Has __init__.py (traditional package) or contains .py files (namespace)
                        if path.join("__init__.py").exists() {
                            return true;
                        }
                        // Check for .py files in the directory
                        std::fs::read_dir(&path)
                            .map(|entries| entries.filter_map(Result::ok).any(|e| {
                                e.path().extension().and_then(|ext| ext.to_str()) == Some("py")
                            }))
                            .unwrap_or(false)
                    } else {
                        // Direct .py file in src/
                        path.extension().and_then(|ext| ext.to_str()) == Some("py")
                    }
                })
            } else {
                false
            };

            if has_python_content {
                let canonical = candidate.canonicalize().unwrap_or(candidate);
                info!("src/ heuristic: detected source root: {}", canonical.display());
                roots.push(canonical);
                break; // Only one src/ dir per project
            }
        }

        roots
    }

    /// Discover source roots by reading pyproject.toml files.
    /// Handles UV workspaces (multi-member) and single projects.
    fn discover_source_roots_from_pyproject(&self) -> Vec<PathBuf> {
        let pyproject_path = self.project_root.join("pyproject.toml");
        let pyproject = match parse_pyproject_toml(&pyproject_path) {
            Some(p) => p,
            None => return Vec::new(),
        };

        let mut roots = Vec::new();

        // Check for UV workspace
        if let Some(tool) = &pyproject.tool {
            if let Some(uv) = &tool.uv {
                if let Some(workspace) = &uv.workspace {
                    if let Some(members) = &workspace.members {
                        info!("pyproject.toml: UV workspace with members: {:?}", members);
                        let member_dirs = expand_workspace_members(&self.project_root, members);
                        for member_dir in &member_dirs {
                            if let Some(root) = extract_source_root(member_dir) {
                                if !roots.contains(&root) {
                                    roots.push(root);
                                }
                            } else {
                                // Fallback: try src/ heuristic on member dir
                                let src_dir = member_dir.join("src");
                                if src_dir.is_dir() && !src_dir.join("__init__.py").exists() {
                                    let has_python = std::fs::read_dir(&src_dir)
                                        .map(|entries| entries.filter_map(Result::ok).any(|e| {
                                            let p = e.path();
                                            (p.is_dir() && (p.join("__init__.py").exists() ||
                                                std::fs::read_dir(&p)
                                                    .map(|es| es.filter_map(Result::ok).any(|f| {
                                                        f.path().extension().and_then(|ext| ext.to_str()) == Some("py")
                                                    }))
                                                    .unwrap_or(false)))
                                            || (p.is_file() && p.extension().and_then(|ext| ext.to_str()) == Some("py"))
                                        }))
                                        .unwrap_or(false);
                                    if has_python {
                                        let canonical = src_dir.canonicalize().unwrap_or(src_dir);
                                        if !roots.contains(&canonical) {
                                            info!("UV workspace member {}: src/ heuristic → {}",
                                                  member_dir.display(), canonical.display());
                                            roots.push(canonical);
                                        }
                                    }
                                }
                            }
                        }

                        if !roots.is_empty() {
                            return roots;
                        }
                    }
                }
            }
        }

        // Single project: try to extract source root
        if let Some(root) = extract_source_root(&self.project_root) {
            roots.push(root);
        }

        roots
    }

    /// Legacy source root detection: depth-1 directories that contain Python packages.
    /// A directory is a source root if it has at least one child directory with `__init__.py`.
    /// Falls back to namespace package detection if no `__init__.py` roots found.
    fn detect_source_roots_legacy(&self) -> Vec<PathBuf> {
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

        // Convert base_path to a module string.
        // Try source roots first (for src/ layout), then fall back to project root.
        let rel_path = self.source_roots.iter()
            .filter_map(|sr| base_path.strip_prefix(sr).ok())
            .next()
            .or_else(|| base_path.strip_prefix(&self.project_root).ok());
        let rel_path = match rel_path {
            Some(p) => p,
            None => return None,
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
    /// `depth` controls recursion: 0 = normal (will follow __init__.py re-exports),
    /// 1+ = called from follow_init_reexports (won't recurse further).
    fn resolve_from_import(
        &self,
        node: tree_sitter::Node,
        current_file: &Path,
        source: &[u8],
        imports: &mut HashSet<PathBuf>,
        depth: usize,
    ) {
        let module_name_node = node.child_by_field_name("module_name");

        match module_name_node {
            Some(mn) if mn.kind() == "dotted_name" => {
                // Absolute: `from app.models import User`
                let text = mn.utf8_text(source).unwrap_or("");
                let root_module = text.split('.').next().unwrap_or("");
                if !PYTHON_STDLIB_MODULES.contains(root_module)
                    && !self.third_party_packages.contains(root_module)
                {
                    if let Some(path) = self.resolve_absolute(text) {
                        if depth == 0 && path.ends_with("__init__.py") {
                            self.follow_init_reexports(&path, imports);
                        }
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
                        if depth == 0 && path.ends_with("__init__.py") {
                            self.follow_init_reexports(&path, imports);
                        }
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
                            if depth == 0 && path.ends_with("__init__.py") {
                                self.follow_init_reexports(&path, imports);
                            }
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
                                    if depth == 0 && path.ends_with("__init__.py") {
                                        self.follow_init_reexports(&path, imports);
                                    }
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
                                if depth == 0 && path.ends_with("__init__.py") {
                                    self.follow_init_reexports(&path, imports);
                                }
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

    /// Parse an `__init__.py` file and follow its from-imports to discover
    /// re-exported submodules. Adds resolved paths as additional edges.
    /// Only goes one level deep (won't recurse into nested `__init__.py`).
    fn follow_init_reexports(&self, init_path: &Path, imports: &mut HashSet<PathBuf>) {
        let source = match std::fs::read(init_path) {
            Ok(s) => s,
            Err(_) => return,
        };
        if source.is_empty() {
            return;
        }

        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(tree_sitter_python::language())
            .expect("Failed to set Python language");
        let tree = match parser.parse(&source, None) {
            Some(t) => t,
            None => return,
        };

        let mut query_cursor = QueryCursor::new();
        let matches = query_cursor.matches(&self.import_query, tree.root_node(), source.as_slice());

        for match_ in matches {
            for capture in match_.captures {
                let capture_name = &self.import_query.capture_names()[capture.index as usize];
                if capture_name == "from_import" {
                    self.resolve_from_import(
                        capture.node,
                        init_path,
                        &source,
                        imports,
                        1, // depth=1 prevents further recursion
                    );
                }
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
                        || self.third_party_packages.contains(root_module)
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
                            || self.third_party_packages.contains(root_module)
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
                    || self.third_party_packages.contains(root_module)
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
                            && !self.third_party_packages.contains(root_module)
                        {
                            self.attempted_imports.fetch_add(1, Ordering::Relaxed);
                            if let Some(path) = self.resolve_absolute(text) {
                                imports.insert(path);
                            } else {
                                self.failed_imports.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    "from_import" => {
                        // Pattern 1: entire `from ... import ...` statement
                        self.attempted_imports.fetch_add(1, Ordering::Relaxed);
                        let before = imports.len();
                        self.resolve_from_import(
                            capture.node,
                            current_file,
                            source,
                            &mut imports,
                            0,
                        );
                        if imports.len() == before {
                            self.failed_imports.fetch_add(1, Ordering::Relaxed);
                        }
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

    fn get_known_root_modules(&self) -> Vec<String> {
        let mut roots: HashSet<String> = HashSet::new();
        for key in self.module_index.keys() {
            if let Some(first) = key.split('.').next() {
                roots.insert(first.to_string());
            }
        }
        let mut sorted: Vec<String> = roots.into_iter().collect();
        sorted.sort();
        sorted
    }

    fn get_attempted_imports(&self) -> usize {
        self.attempted_imports.load(Ordering::Relaxed)
    }

    fn get_failed_imports(&self) -> usize {
        self.failed_imports.load(Ordering::Relaxed)
    }

    fn get_third_party_count(&self) -> usize {
        self.third_party_packages.len()
    }

    fn debug_module_lookup(&self, module_path: &str) -> Option<PathBuf> {
        self.resolve_absolute(module_path)
    }

    fn debug_resolve_import(&self, import_source: &str, current_file: &Path) -> Vec<(String, String)> {
        let mut steps = Vec::new();

        // Show source roots
        steps.push((
            "source_roots".to_string(),
            format!("{:?}", self.source_roots),
        ));

        // Parse the import source as a from-import statement
        let code = format!("{}\n", import_source);
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(tree_sitter_python::language()).unwrap();
        let tree = match parser.parse(code.as_bytes(), None) {
            Some(t) => t,
            None => {
                steps.push(("parse_error".to_string(), "Failed to parse".to_string()));
                return steps;
            }
        };

        // Run import query
        let mut query_cursor = QueryCursor::new();
        let matches = query_cursor.matches(&self.import_query, tree.root_node(), code.as_bytes());

        let mut found_from_import = false;
        for match_ in matches {
            for capture in match_.captures {
                let capture_name = &self.import_query.capture_names()[capture.index as usize];
                match capture_name.as_str() {
                    "from_import" => {
                        found_from_import = true;
                        let node = capture.node;

                        // Extract module_name info
                        let module_name_node = node.child_by_field_name("module_name");
                        match module_name_node {
                            Some(mn) => {
                                let text = mn.utf8_text(code.as_bytes()).unwrap_or("?");
                                steps.push((
                                    "module_name".to_string(),
                                    format!("kind={}, text={}", mn.kind(), text),
                                ));

                                if mn.kind() == "dotted_name" {
                                    // Absolute import
                                    let root_mod = text.split('.').next().unwrap_or("");
                                    let is_stdlib = PYTHON_STDLIB_MODULES.contains(root_mod);
                                    let is_third = self.third_party_packages.contains(root_mod);
                                    steps.push((
                                        "filter_check".to_string(),
                                        format!("root={}, stdlib={}, third_party={}", root_mod, is_stdlib, is_third),
                                    ));

                                    if !is_stdlib && !is_third {
                                        // Try resolve
                                        match self.resolve_absolute(text) {
                                            Some(p) => steps.push((
                                                "resolve_absolute".to_string(),
                                                format!("OK → {}", p.display()),
                                            )),
                                            None => {
                                                // Show what's in the index for nearby keys
                                                let prefix = text.split('.').take(2).collect::<Vec<_>>().join(".");
                                                let nearby: Vec<_> = self.module_index.keys()
                                                    .filter(|k| k.starts_with(&prefix))
                                                    .take(10)
                                                    .cloned()
                                                    .collect();
                                                steps.push((
                                                    "resolve_absolute".to_string(),
                                                    format!("FAILED for '{}'. Nearby index entries with prefix '{}': {:?}", text, prefix, nearby),
                                                ));
                                            }
                                        }
                                    }
                                } else if mn.kind() == "relative_import" {
                                    // Relative import — trace resolution
                                    steps.push((
                                        "relative_import".to_string(),
                                        format!("current_file={}", current_file.display()),
                                    ));

                                    let mut dots = 0usize;
                                    let mut module_path: Option<String> = None;
                                    let mut cursor = mn.walk();
                                    for child in mn.children(&mut cursor) {
                                        match child.kind() {
                                            "import_prefix" => {
                                                dots = child.utf8_text(code.as_bytes()).unwrap_or("").chars().filter(|&c| c == '.').count();
                                            }
                                            "dotted_name" => {
                                                module_path = Some(child.utf8_text(code.as_bytes()).unwrap_or("").to_string());
                                            }
                                            _ => {}
                                        }
                                    }
                                    steps.push((
                                        "relative_parts".to_string(),
                                        format!("dots={}, module={:?}", dots, module_path),
                                    ));

                                    match self.resolve_relative(current_file, dots, module_path.as_deref()) {
                                        Some(p) => steps.push((
                                            "resolve_relative".to_string(),
                                            format!("OK → {}", p.display()),
                                        )),
                                        None => steps.push((
                                            "resolve_relative".to_string(),
                                            "FAILED".to_string(),
                                        )),
                                    }
                                }
                            }
                            None => {
                                steps.push(("module_name".to_string(), "NONE".to_string()));
                            }
                        }

                        // Now do the actual resolution
                        let mut resolved = HashSet::new();
                        self.resolve_from_import(node, current_file, code.as_bytes(), &mut resolved, 0);
                        let resolved_strs: Vec<String> = resolved.iter().map(|p| p.display().to_string()).collect();
                        steps.push((
                            "final_resolved".to_string(),
                            format!("{:?}", resolved_strs),
                        ));
                    }
                    "module" => {
                        let text = capture.node.utf8_text(code.as_bytes()).unwrap_or("?");
                        steps.push(("bare_import".to_string(), text.to_string()));
                        match self.resolve_absolute(text) {
                            Some(p) => steps.push(("resolve".to_string(), format!("OK → {}", p.display()))),
                            None => steps.push(("resolve".to_string(), format!("FAILED for '{}'", text))),
                        }
                    }
                    _ => {}
                }
            }
        }

        if !found_from_import {
            steps.push(("warning".to_string(), "No from-import found in parsed code".to_string()));
        }

        steps
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
    /// Third-party package names from package.json (+ Node built-ins).
    third_party_packages: HashSet<String>,
}

impl JsTsImportResolver {
    pub fn new(project_root: &Path, ignored_dirs: &[String]) -> Self {
        let ignored_set: HashSet<String> = ignored_dirs.iter().cloned().collect();
        let canonical_root = project_root.canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());
        let third_party_packages = Self::discover_third_party_packages(&canonical_root);
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
            third_party_packages,
        };

        resolver.load_tsconfig();
        resolver.index_modules();
        resolver
    }

    // -----------------------------------------------------------------------
    // Third-party package discovery (JS/TS)
    // -----------------------------------------------------------------------

    /// Extract dependency names from a package.json file.
    fn parse_package_json(path: &Path) -> HashSet<String> {
        let mut packages = HashSet::new();
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return packages,
        };
        let pkg: PackageJson = match serde_json::from_str(&content) {
            Ok(p) => p,
            Err(e) => {
                debug!("Cannot parse {}: {}", path.display(), e);
                return packages;
            }
        };
        for deps in [&pkg.dependencies, &pkg.dev_dependencies, &pkg.peer_dependencies] {
            if let Some(deps) = deps {
                for key in deps.keys() {
                    packages.insert(key.clone());
                    // For scoped packages like @babel/core, also add the scope prefix
                    // so `@babel/preset-env` is recognized as third-party when imported
                    if let Some(scope) = key.strip_prefix('@') {
                        if let Some(scope_name) = scope.split('/').next() {
                            packages.insert(format!("@{}", scope_name));
                        }
                    }
                }
            }
        }
        packages
    }

    /// Discover third-party packages from package.json files.
    /// Merges with NODE_BUILTIN_MODULES as a base set.
    fn discover_third_party_packages(project_root: &Path) -> HashSet<String> {
        let mut packages: HashSet<String> = NODE_BUILTIN_MODULES.iter()
            .map(|s| s.to_string())
            .collect();

        let mut discovered = 0usize;

        // package.json in project root
        let pkg_path = project_root.join("package.json");
        if pkg_path.is_file() {
            let found = Self::parse_package_json(&pkg_path);
            discovered += found.len();
            debug!("Found {} packages in {}", found.len(), pkg_path.display());
            packages.extend(found);
        }

        // One level deep: */package.json (monorepo packages)
        if let Ok(entries) = std::fs::read_dir(project_root) {
            for entry in entries.filter_map(Result::ok) {
                let p = entry.path();
                if p.is_dir() {
                    let sub_pkg = p.join("package.json");
                    if sub_pkg.is_file() {
                        let found = Self::parse_package_json(&sub_pkg);
                        discovered += found.len();
                        debug!("Found {} packages in {}", found.len(), sub_pkg.display());
                        packages.extend(found);
                    }
                }
            }
        }

        // Monorepo: packages/*/package.json and apps/*/package.json
        for subdir in &["packages", "apps", "libs"] {
            let mono_dir = project_root.join(subdir);
            if mono_dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&mono_dir) {
                    for entry in entries.filter_map(Result::ok) {
                        let p = entry.path();
                        if p.is_dir() {
                            let sub_pkg = p.join("package.json");
                            if sub_pkg.is_file() {
                                let found = Self::parse_package_json(&sub_pkg);
                                discovered += found.len();
                                debug!("Found {} packages in {}", found.len(), sub_pkg.display());
                                packages.extend(found);
                            }
                        }
                    }
                }
            }
        }

        if discovered > 0 {
            info!("JS/TS third-party packages: {} total ({} from package.json, {} built-in)",
                  packages.len(), discovered, NODE_BUILTIN_MODULES.len());
        }

        packages
    }

    /// Check if a bare import specifier is a known third-party package.
    fn is_third_party_package(&self, specifier: &str) -> bool {
        // Direct match: "react", "lodash", "@babel/core"
        if self.third_party_packages.contains(specifier) {
            return true;
        }
        // Check if it's a subpath import: "react/jsx-runtime" → check "react"
        // or "@scope/pkg/sub" → check "@scope/pkg"
        if specifier.starts_with('@') {
            // Scoped: @scope/pkg/sub → @scope/pkg
            let parts: Vec<&str> = specifier.splitn(3, '/').collect();
            if parts.len() >= 2 {
                let scoped_name = format!("{}/{}", parts[0], parts[1]);
                if self.third_party_packages.contains(&scoped_name) {
                    return true;
                }
            }
            // Also check just the scope: @scope
            if let Some(scope) = specifier.split('/').next() {
                if self.third_party_packages.contains(scope) {
                    return true;
                }
            }
        } else {
            // Unscoped: "lodash/merge" → check "lodash"
            let root = specifier.split('/').next().unwrap_or(specifier);
            if self.third_party_packages.contains(root) {
                return true;
            }
        }
        false
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
        // 1. Handle bare specifiers (not starting with . or /)
        if !specifier.starts_with('.') && !specifier.starts_with('/') {
            // Try to resolve as a path alias first
            if let Some(resolved) = self.resolve_path_alias(specifier) {
                return Some(resolved);
            }
            // Known third-party or Node built-in — skip
            // For bare specifiers not in our known set, still skip (likely node_modules)
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

    fn get_third_party_count(&self) -> usize {
        self.third_party_packages.len()
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

    // -----------------------------------------------------------------------
    // Test Case F: src/ layout without __init__.py → detected as source root
    // -----------------------------------------------------------------------
    #[test]
    fn test_src_layout_without_init() {
        let root = tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();

        // src/mypackage/ is a real package, src/ itself has no __init__.py
        create_dummy_file(&root_path, "src/mypackage/__init__.py", "");
        create_dummy_file(&root_path, "src/mypackage/core.py", "x = 1");

        let resolver = PythonImportResolver::new(&root_path, &[], None);

        // src/ should be detected as a source root (by the heuristic)
        assert_eq!(resolver.source_roots.len(), 1, "Should detect src/ as source root");
        assert_eq!(resolver.source_roots[0], root_path.join("src"));

        // Module index should have source-root-relative keys: mypackage, mypackage.core
        assert!(
            resolver.module_index.contains_key("mypackage"),
            "Should index 'mypackage' relative to src/. Keys: {:?}",
            resolver.module_index.keys().collect::<Vec<_>>()
        );
        assert!(
            resolver.module_index.contains_key("mypackage.core"),
            "Should index 'mypackage.core' relative to src/. Keys: {:?}",
            resolver.module_index.keys().collect::<Vec<_>>()
        );
    }

    // -----------------------------------------------------------------------
    // Test Case G: src/__init__.py exists → NOT a source root (it's a package)
    // -----------------------------------------------------------------------
    #[test]
    fn test_src_as_package_not_source_root() {
        let root = tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();

        // src/ IS a package (has __init__.py)
        create_dummy_file(&root_path, "src/__init__.py", "");
        create_dummy_file(&root_path, "src/utils.py", "x = 1");

        let resolver = PythonImportResolver::new(&root_path, &[], None);

        // src/ heuristic should skip it. Legacy should detect project root
        // as having src/ as a depth-1 dir with __init__.py → no additional source root
        // because src/ is a package itself, not a root containing packages.
        // The module key should be "src.utils", NOT "utils"
        assert!(
            resolver.module_index.contains_key("src.utils"),
            "Should index 'src.utils' (src/ is a package). Keys: {:?}",
            resolver.module_index.keys().collect::<Vec<_>>()
        );
    }

    // -----------------------------------------------------------------------
    // Test Case A: Hatch packages in pyproject.toml
    // -----------------------------------------------------------------------
    #[test]
    fn test_src_layout_with_pyproject_hatch() {
        let root = tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();

        create_dummy_file(&root_path, "src/mypackage/__init__.py", "");
        create_dummy_file(&root_path, "src/mypackage/core.py", "x = 1");
        create_dummy_file(&root_path, "pyproject.toml", r#"
[tool.hatch.build.targets.wheel]
packages = ["src/mypackage"]
"#);

        let resolver = PythonImportResolver::new(&root_path, &[], None);

        assert_eq!(resolver.source_roots.len(), 1, "Should detect src/ as source root via Hatch");
        assert_eq!(resolver.source_roots[0], root_path.join("src"));
        assert!(
            resolver.module_index.contains_key("mypackage.core"),
            "Should index 'mypackage.core' relative to src/. Keys: {:?}",
            resolver.module_index.keys().collect::<Vec<_>>()
        );
    }

    // -----------------------------------------------------------------------
    // Test Case B: UV workspace with member dirs
    // -----------------------------------------------------------------------
    #[test]
    fn test_uv_workspace_members() {
        let root = tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();

        // Root pyproject.toml with UV workspace
        create_dummy_file(&root_path, "pyproject.toml", r#"
[tool.uv.workspace]
members = ["packages/*"]
"#);

        // Member 1: packages/core with src/ layout
        create_dummy_file(&root_path, "packages/core/pyproject.toml", r#"
[tool.hatch.build.targets.wheel]
packages = ["src/core_lib"]
"#);
        create_dummy_file(&root_path, "packages/core/src/core_lib/__init__.py", "");
        create_dummy_file(&root_path, "packages/core/src/core_lib/utils.py", "x = 1");

        // Member 2: packages/api with src/ layout (no tool config, relies on heuristic)
        create_dummy_file(&root_path, "packages/api/pyproject.toml", r#"
[project]
name = "api"
"#);
        create_dummy_file(&root_path, "packages/api/src/api_lib/__init__.py", "");
        create_dummy_file(&root_path, "packages/api/src/api_lib/routes.py", "x = 1");

        let resolver = PythonImportResolver::new(&root_path, &[], None);

        // Should detect two source roots
        assert_eq!(
            resolver.source_roots.len(), 2,
            "Should detect 2 source roots (one per workspace member). Got: {:?}",
            resolver.source_roots
        );

        // Module keys should be relative to source roots
        assert!(
            resolver.module_index.contains_key("core_lib.utils"),
            "Should index 'core_lib.utils'. Keys: {:?}",
            resolver.module_index.keys().collect::<Vec<_>>()
        );
        assert!(
            resolver.module_index.contains_key("api_lib.routes"),
            "Should index 'api_lib.routes'. Keys: {:?}",
            resolver.module_index.keys().collect::<Vec<_>>()
        );
    }

    // -----------------------------------------------------------------------
    // Test Case H: Setuptools package-dir
    // -----------------------------------------------------------------------
    #[test]
    fn test_setuptools_package_dir() {
        let root = tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();

        create_dummy_file(&root_path, "src/mylib/__init__.py", "");
        create_dummy_file(&root_path, "src/mylib/core.py", "x = 1");
        create_dummy_file(&root_path, "pyproject.toml", r#"
[tool.setuptools.package-dir]
"" = "src"
"#);

        let resolver = PythonImportResolver::new(&root_path, &[], None);

        assert_eq!(resolver.source_roots.len(), 1, "Should detect src/ via setuptools");
        assert_eq!(resolver.source_roots[0], root_path.join("src"));
        assert!(
            resolver.module_index.contains_key("mylib.core"),
            "Should index 'mylib.core'. Keys: {:?}",
            resolver.module_index.keys().collect::<Vec<_>>()
        );
    }

    // -----------------------------------------------------------------------
    // Test Case I: Poetry packages.from
    // -----------------------------------------------------------------------
    #[test]
    fn test_poetry_packages_from() {
        let root = tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();

        create_dummy_file(&root_path, "src/mylib/__init__.py", "");
        create_dummy_file(&root_path, "src/mylib/core.py", "x = 1");
        create_dummy_file(&root_path, "pyproject.toml", r#"
[tool.poetry]
name = "mylib"

[[tool.poetry.packages]]
include = "mylib"
from = "src"
"#);

        let resolver = PythonImportResolver::new(&root_path, &[], None);

        assert_eq!(resolver.source_roots.len(), 1, "Should detect src/ via Poetry");
        assert_eq!(resolver.source_roots[0], root_path.join("src"));
        assert!(
            resolver.module_index.contains_key("mylib.core"),
            "Should index 'mylib.core'. Keys: {:?}",
            resolver.module_index.keys().collect::<Vec<_>>()
        );
    }

    // -----------------------------------------------------------------------
    // Test: Relative import in src/ layout
    // -----------------------------------------------------------------------
    #[test]
    fn test_relative_import_in_src_layout() {
        let root = tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();

        create_dummy_file(&root_path, "src/mypackage/__init__.py", "");
        create_dummy_file(&root_path, "src/mypackage/utils.py", "def helper(): pass");
        create_dummy_file(&root_path, "src/mypackage/core.py", "from . import utils");

        let mut parser = Parser::new();
        parser.set_language(tree_sitter_python::language()).unwrap();

        let resolver = PythonImportResolver::new(&root_path, &[], None);

        let source = "from . import utils";
        let tree = parser.parse(source, None).unwrap();
        let core_path = root_path.join("src/mypackage/core.py");
        let imports = resolver.find_imports(&tree, &core_path, source.as_bytes());

        let utils_path = root_path.join("src/mypackage/utils.py");
        assert!(
            imports.contains(&utils_path),
            "Relative import 'from . import utils' in src/ layout should resolve.\n\
             Expected: {:?}\n\
             Got: {:?}\n\
             Source roots: {:?}",
            utils_path,
            imports,
            resolver.source_roots,
        );
    }

    // -----------------------------------------------------------------------
    // Test: Double-dot relative import in src/ layout
    // -----------------------------------------------------------------------
    #[test]
    fn test_relative_import_double_dot_in_src_layout() {
        let root = tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();

        create_dummy_file(&root_path, "src/mypackage/__init__.py", "");
        create_dummy_file(&root_path, "src/mypackage/sub/__init__.py", "");
        create_dummy_file(&root_path, "src/mypackage/sub/deep.py", "from .. import utils");
        create_dummy_file(&root_path, "src/mypackage/utils.py", "def helper(): pass");

        let mut parser = Parser::new();
        parser.set_language(tree_sitter_python::language()).unwrap();

        let resolver = PythonImportResolver::new(&root_path, &[], None);

        let source = "from .. import utils";
        let tree = parser.parse(source, None).unwrap();
        let deep_path = root_path.join("src/mypackage/sub/deep.py");
        let imports = resolver.find_imports(&tree, &deep_path, source.as_bytes());

        let utils_path = root_path.join("src/mypackage/utils.py");
        assert!(
            imports.contains(&utils_path),
            "Relative import 'from .. import utils' in src/ layout should resolve.\n\
             Expected: {:?}\n\
             Got: {:?}\n\
             Source roots: {:?}",
            utils_path,
            imports,
            resolver.source_roots,
        );
    }

    // -----------------------------------------------------------------------
    // Test: Setuptools find.where
    // -----------------------------------------------------------------------
    #[test]
    fn test_setuptools_find_where() {
        let root = tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();

        create_dummy_file(&root_path, "src/mylib/__init__.py", "");
        create_dummy_file(&root_path, "src/mylib/core.py", "x = 1");
        create_dummy_file(&root_path, "pyproject.toml", r#"
[tool.setuptools.packages.find]
where = ["src"]
"#);

        let resolver = PythonImportResolver::new(&root_path, &[], None);

        assert_eq!(resolver.source_roots.len(), 1, "Should detect src/ via setuptools find.where");
        assert_eq!(resolver.source_roots[0], root_path.join("src"));
        assert!(
            resolver.module_index.contains_key("mylib.core"),
            "Should index 'mylib.core'. Keys: {:?}",
            resolver.module_index.keys().collect::<Vec<_>>()
        );
    }
}