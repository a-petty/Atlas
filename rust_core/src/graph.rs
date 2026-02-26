use crate::cpg::CpgLayer;
use crate::import_resolver::{ImportResolver, JsTsImportResolver, PythonImportResolver};
use crate::parser::{self, Symbol, SymbolHarvester, SupportedLanguage};
use crate::symbol_table::SymbolIndex;
use log::{debug, info, warn};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use rayon::prelude::*;
use thiserror::Error;
use std::sync::Arc;
use lru::LruCache;
use std::num::NonZeroUsize;
use parking_lot::RwLock;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::fmt::Write;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tree_sitter::Parser as TreeSitterParser;

/// Represents an error that can occur during graph operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GraphError {
    #[error("Node not found for path: {0}")]
    NodeNotFound(PathBuf),
    #[error("File I/O error: {0}")]
    IoError(String),
    #[error("Parsing failed for file: {0}")]
    ParseError(PathBuf),
    #[error("Unsupported language for file: {0}")]
    UnsupportedLanguage(PathBuf),
}

impl From<std::io::Error> for GraphError {
    fn from(err: std::io::Error) -> Self {
        GraphError::IoError(err.to_string())
    }
}

/// Result of a graph update operation
#[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
pub struct UpdateResult {
    pub edges_added: usize,
    pub edges_removed: usize,
    pub needs_pagerank_recalc: bool,
}

/// Represents a node in the repository graph, which is a single source file.
#[derive(Debug, Clone)]
pub struct FileNode {
    pub path: PathBuf,
    pub language: SupportedLanguage,
    pub definitions: Vec<String>,
    pub usages: Vec<String>,
    pub rank: f64,

    // Cached data for fast change classification
    pub imports_hash: u64,
    pub definitions_hash: u64,
    pub usages_hash: u64,
    pub content_hash: u64,
}

impl FileNode {
    /// Creates a new FileNode with computed hashes
    pub fn new(
        path: PathBuf,
        language: SupportedLanguage,
        definitions: Vec<String>,
        usages: Vec<String>,
        imports: &HashSet<PathBuf>,
        content: &str,
    ) -> Self {
        Self {
            path,
            language,
            definitions: definitions.clone(),
            usages: usages.clone(),
            rank: 0.0,
            imports_hash: Self::hash_imports(imports),
            definitions_hash: Self::hash_vec(&definitions),
            usages_hash: Self::hash_vec(&usages),
            content_hash: Self::hash_content(content),
        }
    }

    /// Creates an empty node (used when initializing before full parse)
    pub fn empty(path: PathBuf) -> Self {
        let language = SupportedLanguage::from_path(&path);
        Self {
            path,
            language,
            definitions: Vec::new(),
            usages: Vec::new(),
            rank: 0.0,
            imports_hash: 0,
            definitions_hash: 0,
            usages_hash: 0,
            content_hash: 0,
        }
    }

    pub fn hash_imports(imports: &HashSet<PathBuf>) -> u64 {
        let mut hasher = DefaultHasher::new();
        let mut sorted: Vec<_> = imports.iter().collect();
        sorted.sort(); // Ensure deterministic hashing
        for import in sorted {
            import.hash(&mut hasher);
        }
        hasher.finish()
    }

    pub fn hash_vec(vec: &[String]) -> u64 {
        let mut hasher = DefaultHasher::new();
        let mut sorted = vec.to_vec();
        sorted.sort();
        for item in sorted {
            item.hash(&mut hasher);
        }
        hasher.finish()
    }

    pub fn hash_content(content: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        hasher.finish()
    }
}

/// Represents the type of dependency between two files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeKind {
    /// A heuristic dependency based on symbol name matching.
    SymbolUsage,
    /// A structurally confirmed dependency based on an import statement.
    Import,
}

impl EdgeKind {
    pub fn strength(&self) -> f64 {
        match self {
            EdgeKind::Import => 2.0,       // Strong: structurally confirmed via AST
            EdgeKind::SymbolUsage => 1.0,   // Weak: heuristic name matching
        }
    }
}

/// Maximum number of files a symbol can be defined in before it's considered noise.
/// Symbols defined in more files than this are skipped during SymbolUsage edge creation.
const MAX_SYMBOL_DEFINITION_FILES: usize = 5;

/// Check if two languages are compatible for SymbolUsage edges.
/// JS/TS variants can reference each other, but Python↔JS/TS etc. cannot.
fn languages_compatible(a: SupportedLanguage, b: SupportedLanguage) -> bool {
    use SupportedLanguage::*;
    matches!(
        (a, b),
        (Python, Python)
            | (JavaScript | JavaScriptJsx, JavaScript | JavaScriptJsx | TypeScript | TypeScriptTsx)
            | (TypeScript | TypeScriptTsx, JavaScript | JavaScriptJsx | TypeScript | TypeScriptTsx)
            | (Rust, Rust)
            | (Go, Go)
    )
}

/// Represents the magnitude of a change to a file, used for tiered updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateTier {
    /// Only the function body changed. No change to symbols or imports.
    Local,
    /// The file's symbols changed (e.g., a function was added/removed).
    FileScope,
    /// An import statement was changed, affecting graph structure.
    GraphScope,
    /// Change is too significant or affects fundamental structure, requiring a full rebuild or more extensive processing.
    FullRebuild,
}

/// Statistics about the repository graph
#[derive(Debug)]
pub struct GraphStatistics {
    pub node_count: usize,
    pub edge_count: usize,
    pub import_edges: usize,
    pub symbol_edges: usize,
    pub total_definitions: usize,
    pub total_files_with_usages: usize,
    pub unresolved_import_count: usize,
    pub source_roots: Vec<PathBuf>,
    pub module_index_size: usize,
}

/// A temporary, lightweight container for the results of parsing a single file.
/// This is designed to be created in parallel and then integrated serially.
struct FileParseResult {
    path: PathBuf,
    symbols: Vec<parser::Symbol>,
    imports: HashSet<PathBuf>,
    content_hash: u64,
}

#[derive(Debug)]
struct DirTree {
    files: Vec<(PathBuf, f64)>, // Files in this directory with their ranks
    subdirs: BTreeMap<String, DirTree>, // Subdirectories
}

impl DirTree {
    fn new() -> Self {
        Self {
            files: Vec::new(),
            subdirs: BTreeMap::new(),
        }
    }
}

/// The main repository graph, representing the entire codebase's structure.
#[derive(Debug)]
pub struct RepoGraph {
    /// The `petgraph` directed graph.
    pub graph: DiGraph<FileNode, EdgeKind>,
    /// A map from file paths to their corresponding node indices in the graph.
    pub path_to_idx: HashMap<PathBuf, NodeIndex>,
    /// The global index of all symbols in the repository.
    pub symbol_index: SymbolIndex,
    pub import_resolver: Box<dyn ImportResolver>,
    pub symbol_harvester: SymbolHarvester,
    /// Flag to indicate if PageRank needs recalculation.
    pub pagerank_dirty: bool,
    /// NEW: Maps import target paths to nodes waiting for them
    /// Example: {"utils/logger.py" -> {node_5, node_8}}
    pub unresolved_imports: HashMap<PathBuf, HashSet<NodeIndex>>,
    pub project_root: PathBuf,
    /// Lazy skeleton cache
    pub skeleton_cache: RwLock<LruCache<PathBuf, Arc<String>>>,
    /// Optional CPG overlay for sub-file granularity
    pub cpg: Option<CpgLayer>,
}

impl RepoGraph {
    pub fn new(project_root: &Path, language: &str, ignored_dirs: &[String], source_roots: Option<&[String]>) -> Self {
        let canonical_root = project_root.canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());
        let import_resolver: Box<dyn ImportResolver> = match language.to_lowercase().as_str() {
            "python" => Box::new(PythonImportResolver::new(&canonical_root, ignored_dirs, source_roots)),
            "javascript" | "typescript" | "js" | "ts" => {
                Box::new(JsTsImportResolver::new(&canonical_root, ignored_dirs))
            }
            _ => panic!("Unsupported language: {}", language),
        };

        let cache_size = NonZeroUsize::new(500).unwrap(); // Cache 500 skeletons

        Self {
            graph: DiGraph::new(),
            path_to_idx: HashMap::new(),
            symbol_index: SymbolIndex::new(),
            import_resolver,
            symbol_harvester: SymbolHarvester::new(),
            pagerank_dirty: true,
            unresolved_imports: HashMap::new(),
            project_root: canonical_root,
            skeleton_cache: RwLock::new(LruCache::new(cache_size)),
            cpg: None,
        }
    }

    /// Enable the CPG overlay layer.
    pub fn enable_cpg(&mut self) {
        self.cpg = Some(CpgLayer::new());
    }

    /// Enable CPG and build data for all files already in the graph.
    /// Used when CPG is enabled after build_complete().
    /// `excluded_prefixes` optionally filters out files under certain directories.
    pub fn enable_cpg_and_build(&mut self, excluded_prefixes: Option<&[String]>) {
        self.cpg = Some(CpgLayer::new());
        let all_count = self.graph.node_count();
        let paths: Vec<PathBuf> = self.graph.node_weights()
            .map(|n| n.path.clone())
            .filter(|p| {
                if let Some(prefixes) = excluded_prefixes {
                    let path_str = p.to_string_lossy();
                    !prefixes.iter().any(|prefix| {
                        // Match directory name as a path component
                        path_str.contains(&format!("/{prefix}/"))
                            || path_str.contains(&format!("\\{prefix}\\"))
                            || path_str.ends_with(&format!("/{prefix}"))
                    })
                } else {
                    true
                }
            })
            .collect();
        let excluded = all_count - paths.len();
        if excluded > 0 {
            info!("CPG build: {} files ({} excluded by ignore rules)", paths.len(), excluded);
        }
        self.build_cpg_for_all_files(&paths);
        self.compute_all_import_bindings();
        if let Some(cpg) = &mut self.cpg {
            crate::callgraph::CallGraphBuilder::resolve_all(cpg, &self.symbol_index);
        }
    }

    /// Build CPG data for a single file on demand (incremental).
    /// Creates the CPG layer if not yet created. Skips if already built for this file.
    /// Returns true if CPG data is available for the file after this call.
    pub fn ensure_cpg_for_file(&mut self, path: &Path) -> bool {
        if self.cpg.is_none() {
            self.cpg = Some(CpgLayer::new());
        }

        let cpg = self.cpg.as_ref().unwrap();
        if cpg.has_file(path) {
            return true;
        }

        let lang = SupportedLanguage::from_path(path);
        if lang != SupportedLanguage::Python {
            return false;
        }

        let ts_lang = match lang.get_parser() {
            Some(l) => l,
            None => return false,
        };

        let source = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => return false,
        };

        let mut parser = TreeSitterParser::new();
        if parser.set_language(ts_lang).is_err() {
            return false;
        }

        let tree = match parser.parse(&source, None) {
            Some(t) => t,
            None => return false,
        };

        if let Some(cpg) = &mut self.cpg {
            cpg.build_file(path, tree, source, lang);
        }
        self.compute_import_bindings_for_file(path);
        if let Some(cpg) = &mut self.cpg {
            crate::callgraph::CallGraphBuilder::resolve_file(cpg, path, &self.symbol_index);
        }

        true
    }

    /// Get skeleton for a file, loading on-demand if not cached
    pub fn get_skeleton(&self, path: &Path) -> Result<Arc<String>, GraphError> {
        let canonical_path = self.project_root.join(path);

        // Check cache first
        {
            let mut cache = self.skeleton_cache.write();
            if let Some(skeleton) = cache.get(&canonical_path) {
                return Ok(Arc::clone(skeleton));
            }
        }

        // Not in cache - load and parse
        let source = fs::read_to_string(&canonical_path)
            .map_err(|e| GraphError::IoError(e.to_string()))?;
        
        let lang = SupportedLanguage::from_path(&canonical_path);
        if lang == SupportedLanguage::Unknown {
            // For unknown languages, skeleton is the source itself.
            let skeleton_arc = Arc::new(source);
            let mut cache = self.skeleton_cache.write();
            cache.put(canonical_path, Arc::clone(&skeleton_arc));
            return Ok(skeleton_arc);
        }

        let mut parser = TreeSitterParser::new();
        let ts_lang = lang.get_parser().ok_or_else(|| GraphError::ParseError(canonical_path.clone()))?;
        parser.set_language(ts_lang).map_err(|_| GraphError::ParseError(canonical_path.clone()))?;

        let tree = parser.parse(&source, None).ok_or_else(|| GraphError::ParseError(canonical_path.clone()))?;
        
        let skeleton = parser::create_skeleton(&source, &tree, lang);
        
        let skeleton_arc = Arc::new(skeleton);
        
        // Store in cache
        {
            let mut cache = self.skeleton_cache.write();
            cache.put(canonical_path.clone(), Arc::clone(&skeleton_arc));
        }
        
        Ok(skeleton_arc)
    }

    /// Helper to ensure an edge exists. Import edges (structurally confirmed) take
    /// priority over SymbolUsage edges (heuristic name matching).
    fn ensure_edge(&mut self, source: NodeIndex, target: NodeIndex, kind: EdgeKind) -> bool {
        if let Some(edge_id) = self.graph.find_edge(source, target) {
            // Edge already exists — upgrade SymbolUsage to Import, but never downgrade
            let existing_kind = &mut self.graph[edge_id];
            if *existing_kind == EdgeKind::SymbolUsage && kind == EdgeKind::Import {
                *existing_kind = EdgeKind::Import;
                return true; // Upgraded
            }
            return false; // No change needed
        } else {
            // No edge exists, add it
            self.graph.add_edge(source, target, kind);
            return true;
        }
    }

    /// Classifies the magnitude of change for a given file.
    /// Compares old definitions/usages and imports with new ones to determine UpdateTier.
    pub fn classify_change(
        &self,
        file_path: &Path,
        new_source: &str,
    ) -> Result<UpdateTier, GraphError> {
        let node_idx = *self
            .path_to_idx
            .get(file_path)
            .ok_or_else(|| GraphError::NodeNotFound(file_path.to_path_buf()))?;

        let old_node = self
            .graph
            .node_weight(node_idx)
            .ok_or_else(|| GraphError::NodeNotFound(file_path.to_path_buf()))?;

        // 1. Quick content check
        let new_content_hash = FileNode::hash_content(new_source);
        if new_content_hash == old_node.content_hash {
            return Ok(UpdateTier::Local);
        }

        // Harvest new symbols and imports from the new source
        let lang = SupportedLanguage::from_extension(
            file_path.extension().and_then(|s| s.to_str()).unwrap_or(""),
        );
        if lang == SupportedLanguage::Unknown {
            return Ok(UpdateTier::Local); // Or some other default for unknown languages
        }
        let mut parser = TreeSitterParser::new();
        parser
            .set_language(
                lang.get_parser()
                    .ok_or_else(|| GraphError::ParseError(file_path.to_path_buf()))?,
            )
            .unwrap();
        let tree = parser
            .parse(new_source, None)
            .ok_or_else(|| GraphError::ParseError(file_path.to_path_buf()))?;

        if tree.root_node().has_error() {
            return Err(GraphError::ParseError(file_path.to_path_buf()));
        }

        let symbols = self.symbol_harvester.harvest(&tree, new_source, lang);
        let new_defs: Vec<String> = symbols.iter().filter(|s| s.is_definition).map(|s| s.name.clone()).collect();
        let new_uses: Vec<String> = symbols.iter().filter(|s| !s.is_definition).map(|s| s.name.clone()).collect();


        // Get NEW imports (using the existing tree)
        let new_import_paths = self.import_resolver.find_imports(&tree, file_path, new_source.as_bytes());

        // Compare hashes
        let new_imports_hash = FileNode::hash_imports(&new_import_paths);
        let new_defs_hash = FileNode::hash_vec(&new_defs);
        let new_usages_hash = FileNode::hash_vec(&new_uses);

        if new_imports_hash != old_node.imports_hash {
            return Ok(UpdateTier::GraphScope);
        }

        if new_defs_hash != old_node.definitions_hash {
            return Ok(UpdateTier::FileScope);
        }

        if new_usages_hash != old_node.usages_hash {
            return Ok(UpdateTier::FileScope);
        }

        Ok(UpdateTier::Local)
    }

    /// Updates a single file in the graph, recalculating its symbols and edges.
    pub fn update_file(
        &mut self,
        file_path: &PathBuf,
        source_code: &str,
    ) -> Result<UpdateResult, GraphError> {
        // Canonicalize path to match build() convention
        let file_path = &file_path.canonicalize().unwrap_or_else(|_| file_path.clone());
        debug!("Updating file: {}", file_path.display());
        let tier = self.classify_change(file_path, source_code)?;
        info!(
            "Change classification for {}: {:?}",
            file_path.display(),
            tier
        );

        if tier == UpdateTier::Local {
            // Update content hash even for local changes
            if let Some(&node_idx) = self.path_to_idx.get(file_path) {
                if let Some(node) = self.graph.node_weight_mut(node_idx) {
                    node.content_hash = FileNode::hash_content(source_code);
                }
            }
            // CPG still needs updating for local changes (byte offsets shift)
            if self.cpg.is_some() {
                let cpg_lang = SupportedLanguage::from_extension(
                    file_path.extension().and_then(|s| s.to_str()).unwrap_or(""),
                );
                if cpg_lang != SupportedLanguage::Unknown {
                    if let Some(ts_lang) = cpg_lang.get_parser() {
                        let mut cpg_parser = TreeSitterParser::new();
                        if cpg_parser.set_language(ts_lang).is_ok() {
                            if let Some(cpg_tree) = cpg_parser.parse(source_code, None) {
                                if let Some(cpg) = &mut self.cpg {
                                    cpg.update_file(file_path, cpg_tree, source_code.to_string(), cpg_lang);
                                }
                            }
                        }
                    }
                }
                // Recompute import bindings and re-resolve call graph for this file
                self.compute_import_bindings_for_file(file_path);
                if let Some(cpg) = &mut self.cpg {
                    crate::callgraph::CallGraphBuilder::resolve_file(cpg, file_path, &self.symbol_index);
                }
            }
            let result = UpdateResult {
                edges_added: 0,
                edges_removed: 0,
                needs_pagerank_recalc: false,
            };
            info!("Update complete for {}: {:?}", file_path.display(), result);
            return Ok(result);
        }

        let node_idx = *self
            .path_to_idx
            .get(file_path)
            .ok_or_else(|| GraphError::NodeNotFound(file_path.clone()))?;

        // 1. Get Old State & Re-harvest Symbols
        let old_node = self
            .graph
            .node_weight(node_idx)
            .cloned()
            .ok_or_else(|| GraphError::NodeNotFound(file_path.clone()))?;
        let lang =
            SupportedLanguage::from_extension(file_path.extension().and_then(|s| s.to_str()).unwrap_or(""));
        if lang == SupportedLanguage::Unknown {
            return Ok(UpdateResult::default());
        }
        let mut parser = TreeSitterParser::new();
        parser
            .set_language(
                lang.get_parser()
                    .ok_or_else(|| GraphError::ParseError(file_path.clone()))?,
            )
            .unwrap();
        let tree = parser
            .parse(source_code, None)
            .ok_or_else(|| GraphError::ParseError(file_path.clone()))?;
                let symbols = self.symbol_harvester.harvest(&tree, source_code, lang);
        let new_defs: Vec<String> = symbols.iter().filter(|s| s.is_definition).map(|s| s.name.clone()).collect();
        let new_uses: Vec<String> = symbols.iter().filter(|s| !s.is_definition).map(|s| s.name.clone()).collect();

        let old_defs_set: HashSet<_> = old_node.definitions.iter().cloned().collect();
        let new_defs_set: HashSet<_> = new_defs.iter().cloned().collect();
        let old_uses_set: HashSet<_> = old_node.usages.iter().cloned().collect();
        let new_uses_set: HashSet<_> = new_uses.iter().cloned().collect();

        let removed_defs = old_defs_set
            .difference(&new_defs_set)
            .cloned()
            .collect::<HashSet<_>>();
        let added_defs = new_defs_set
            .difference(&old_defs_set)
            .cloned()
            .collect::<HashSet<_>>();
        let removed_uses = old_uses_set
            .difference(&new_uses_set)
            .cloned()
            .collect::<HashSet<_>>();
        let added_uses = new_uses_set
            .difference(&old_uses_set)
            .cloned()
            .collect::<HashSet<_>>();

        let mut edges_removed = 0;
        let mut edges_added = 0;

        // 2. Clean up Symbol Index and Graph for Removed Definitions
        for def in &removed_defs {
            // Remove from the main definition index
            if let Some(paths) = self.symbol_index.definitions.get_mut(def) {
                paths.retain(|p| p != file_path);
                if paths.is_empty() {
                    self.symbol_index.definitions.remove(def);
                }
            }
            // Find all files that used this definition and remove the edge
            if let Some(users) = self.symbol_index.users.get(def) {
                for user_path in users {
                    if let Some(&user_idx) = self.path_to_idx.get(user_path) {
                        let edges_to_remove: Vec<_> = self
                            .graph
                            .edges_directed(user_idx, petgraph::Direction::Outgoing)
                            .filter(|e| {
                                e.target() == node_idx && *e.weight() == EdgeKind::SymbolUsage
                            })
                            .map(|e| e.id())
                            .collect();

                        for edge_id in edges_to_remove {
                            self.graph.remove_edge(edge_id);
                            edges_removed += 1;
                        }
                    }
                }
            }
        }

        // 3. Clean up Graph for Removed Usages (Outgoing edges)
        for use_ in &removed_uses {
            if let Some(def_paths) = self.symbol_index.definitions.get(use_) {
                for def_path in def_paths {
                    if let Some(&def_idx) = self.path_to_idx.get(def_path) {
                        let edges_to_remove: Vec<_> = self
                            .graph
                            .edges_directed(node_idx, petgraph::Direction::Outgoing)
                            .filter(|e| {
                                e.target() == def_idx && *e.weight() == EdgeKind::SymbolUsage
                            })
                            .map(|e| e.id())
                            .collect();

                        for edge_id in edges_to_remove {
                            self.graph.remove_edge(edge_id);
                            edges_removed += 1;
                        }
                    }
                }
            }
        }

        let new_import_paths = self.import_resolver.find_imports(&tree, file_path, source_code.as_bytes());

        // 4. Update the node in the graph
        {
            let node = self.graph.node_weight_mut(node_idx).unwrap();
            node.definitions = new_defs.clone();
            node.usages = new_uses.clone();
            // Update hashes
            node.definitions_hash = FileNode::hash_vec(&new_defs);
            node.usages_hash = FileNode::hash_vec(&new_uses);
            node.imports_hash = FileNode::hash_imports(&new_import_paths);
            node.content_hash = FileNode::hash_content(source_code);
        }

        // 5. Update Symbol Index for new definitions and usages
        for def in &added_defs {
            self.symbol_index
                .definitions
                .entry(def.clone())
                .or_default()
                .push(file_path.clone());
        }
        self.symbol_index
            .usages
            .insert(file_path.clone(), new_uses.clone());
        // Update the reverse `users` mapping
        for use_ in &removed_uses {
            if let Some(users) = self.symbol_index.users.get_mut(use_) {
                users.retain(|p| p != file_path);
            }
        }
        for use_ in &added_uses {
            self.symbol_index
                .users
                .entry(use_.clone())
                .or_default()
                .push(file_path.clone());
        }

        // 6. Create new edges for Added Definitions (Incoming, import-aware)
        // Only create SymbolUsage edges from user files that import this file
        let this_lang = lang;
        let mut edges_to_create = Vec::new();
        for def in &added_defs {
            if let Some(def_paths) = self.symbol_index.definitions.get(def) {
                if def_paths.len() > MAX_SYMBOL_DEFINITION_FILES { continue; }
            }
            if let Some(user_paths) = self.symbol_index.users.get(def) {
                for user_path in user_paths {
                    if user_path == file_path {
                        continue;
                    }
                    if let Some(&user_idx) = self.path_to_idx.get(user_path) {
                        // Only if the user file imports this file
                        let has_import = self
                            .graph
                            .edges_directed(user_idx, petgraph::Direction::Outgoing)
                            .any(|e| e.target() == node_idx && *e.weight() == EdgeKind::Import);
                        if has_import {
                            let user_lang = self.graph[user_idx].language;
                            if languages_compatible(user_lang, this_lang) {
                                edges_to_create.push((user_idx, node_idx));
                            }
                        }
                    }
                }
            }
        }
        for (src, dst) in edges_to_create {
            if self.ensure_edge(src, dst, EdgeKind::SymbolUsage) {
                edges_added += 1;
            }
        }

        // 7. Create new edges for Added Usages (Outgoing, import-aware)
        // Pre-compute new import targets from the updated import resolution
        let new_import_target_indices: HashSet<NodeIndex> = new_import_paths
            .iter()
            .filter_map(|p| self.path_to_idx.get(p).copied())
            .collect();

        let mut edges_to_create = Vec::new();
        for use_ in &added_uses {
            if let Some(def_paths) = self.symbol_index.definitions.get(use_) {
                if def_paths.len() > MAX_SYMBOL_DEFINITION_FILES { continue; }
                for def_path in def_paths {
                    if def_path == file_path {
                        continue;
                    }
                    if let Some(&def_idx) = self.path_to_idx.get(def_path) {
                        // Only if this file imports the defining file
                        if !new_import_target_indices.contains(&def_idx) { continue; }
                        let def_lang = self.graph[def_idx].language;
                        if languages_compatible(this_lang, def_lang) {
                            edges_to_create.push((node_idx, def_idx));
                        }
                    }
                }
            }
        }
        for (src, dst) in edges_to_create {
            if self.ensure_edge(src, dst, EdgeKind::SymbolUsage) {
                edges_added += 1;
            }
        }

        // 8. Handle Import Edge Changes (Differential Update)
        // Get OLD imports (current state in graph)
        let old_import_targets: HashSet<NodeIndex> = self
            .graph
            .edges_directed(node_idx, petgraph::Direction::Outgoing)
            .filter(|edge| *edge.weight() == EdgeKind::Import)
            .map(|edge| edge.target())
            .collect();

        let new_import_targets: HashSet<NodeIndex> = new_import_paths
            .iter()
            .filter_map(|path| self.path_to_idx.get(path))
            .copied()
            .filter(|&target_idx| target_idx != node_idx) // Prevent self-imports
            .collect();

        // Calculate differences
        let imports_to_remove: Vec<_> = old_import_targets
            .difference(&new_import_targets)
            .copied()
            .collect();
        let imports_to_add: Vec<_> = new_import_targets
            .difference(&old_import_targets)
            .copied()
            .collect();

        // Remove edges
        for target in imports_to_remove {
            let mut edges_to_remove = Vec::new();
            for edge in self
                .graph
                .edges_directed(node_idx, petgraph::Direction::Outgoing)
            {
                if edge.target() == target && *edge.weight() == EdgeKind::Import {
                    edges_to_remove.push(edge.id());
                }
            }
            for edge_id in edges_to_remove {
                self.graph.remove_edge(edge_id);
                edges_removed += 1;
            }
        }

        // Add edges
        for target in imports_to_add {
            if self.ensure_edge(node_idx, target, EdgeKind::Import) {
                edges_added += 1;
            }
        }

        let needs_recalc = edges_added > 0 || edges_removed > 0;
        if needs_recalc {
            self.pagerank_dirty = true;
        }

        // CPG update: reuse the already-parsed tree
        if self.cpg.is_some() {
            let cpg_lang = SupportedLanguage::from_extension(
                file_path.extension().and_then(|s| s.to_str()).unwrap_or(""),
            );
            if cpg_lang != SupportedLanguage::Unknown {
                if let Some(ts_lang) = cpg_lang.get_parser() {
                    let mut cpg_parser = TreeSitterParser::new();
                    if cpg_parser.set_language(ts_lang).is_ok() {
                        if let Some(cpg_tree) = cpg_parser.parse(source_code, None) {
                            if let Some(cpg) = &mut self.cpg {
                                cpg.update_file(file_path, cpg_tree, source_code.to_string(), cpg_lang);
                            }
                        }
                    }
                }
            }
            // Recompute import bindings and re-resolve call graph for this file
            self.compute_import_bindings_for_file(file_path);
            if let Some(cpg) = &mut self.cpg {
                crate::callgraph::CallGraphBuilder::resolve_file(cpg, file_path, &self.symbol_index);
            }
        }

        let result = UpdateResult {
            edges_added,
            edges_removed,
            needs_pagerank_recalc: needs_recalc,
        };
        info!("Update complete for {}: {:?}", file_path.display(), result);
        Ok(result)
    }

    /// Ensures PageRank scores are up-to-date before any rank-dependent operation.
    pub fn ensure_pagerank_up_to_date(&mut self) {
        if self.pagerank_dirty {
            self.calculate_pagerank(20, 0.85);
        }
    }

    /// Builds the graph from a list of file paths using a parallelized, multi-stage process.
    pub fn build(&mut self, paths: &[PathBuf]) {
        info!("Starting parallel build for {} files...", paths.len());

        // Canonicalize all paths up front so path_to_idx matches import resolver's module_index
        let canonical_paths: Vec<PathBuf> = paths
            .iter()
            .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
            .collect();

        // Stage 1: Parallel File Parsing and Data Collection
        let parse_results: Vec<FileParseResult> = canonical_paths
            .par_iter()
            .filter_map(|path| {
                let lang = SupportedLanguage::from_extension(
                    path.extension().and_then(|s| s.to_str()).unwrap_or(""),
                );
                if lang == SupportedLanguage::Unknown {
                    return None;
                }

                // It's important to initialize the parser per-thread
                let mut parser = TreeSitterParser::new();
                if parser.set_language(lang.get_parser().unwrap()).is_err() {
                    return None;
                };

                if let Ok(source_code) = fs::read_to_string(path) {
                    if let Some(tree) = parser.parse(&source_code, None) {
                        if tree.root_node().has_error() {
                            debug!("Skipping file with syntax errors: {}", path.display());
                            return None;
                        }

                        let symbol_harvester = SymbolHarvester::new(); // Per-thread harvester
                        let symbols = symbol_harvester.harvest(&tree, &source_code, lang);
                        let imports = self.import_resolver.find_imports(&tree, path, source_code.as_bytes());
                        let content_hash = FileNode::hash_content(&source_code);

                        return Some(FileParseResult {
                            path: path.clone(),
                            symbols,
                            imports,
                            content_hash,
                        });
                    }
                }
                None
            })
            .collect();

        info!(
            "Parallel parsing complete. Collected data for {} out of {} files.",
            parse_results.len(),
            paths.len()
        );

        // Stage 2: Serial Integration (mutating self, so must be serial)
        
        // Pass 2.1: Node Creation
        // Create all nodes first so we can reliably create edges later.
        for result in &parse_results {
            let defs: Vec<String> = result.symbols.iter().filter(|s| s.is_definition).map(|s| s.name.clone()).collect();
            let uses: Vec<String> = result.symbols.iter().filter(|s| !s.is_definition).map(|s| s.name.clone()).collect();
            let lang = SupportedLanguage::from_path(&result.path);

            let file_node = FileNode::new(
                result.path.clone(),
                lang,
                defs,
                uses,
                &result.imports,
                "", // Content is not stored in node, hash is used instead
            );
            
            // Overwrite content_hash with the one we already calculated
            let mut file_node_with_hash = file_node;
            file_node_with_hash.content_hash = result.content_hash;

            let node_idx = self.graph.add_node(file_node_with_hash);
            self.path_to_idx.insert(result.path.clone(), node_idx);
        }
        info!("Node creation complete. Graph has {} nodes.", self.graph.node_count());

        // Pass 2.2: Symbol and Import Indexing
        let mut import_map: HashMap<NodeIndex, HashSet<PathBuf>> = HashMap::new();
        for result in parse_results {
            // We need to get the node_idx again as the `parse_results` was consumed in the previous loop
            if let Some(&node_idx) = self.path_to_idx.get(&result.path) {
                let all_uses: Vec<String> = result.symbols.iter()
                    .filter(|s| !s.is_definition)
                    .map(|s| s.name.clone())
                    .collect();

                // Populate symbol index
                for symbol in result.symbols {
                    if symbol.is_definition {
                        self.symbol_index
                            .definitions
                            .entry(symbol.name)
                            .or_default()
                            .push(result.path.clone());
                    } else {
                        self.symbol_index
                            .users
                            .entry(symbol.name.clone())
                            .or_default()
                            .push(result.path.clone());
                    }
                }
                self.symbol_index.usages.insert(result.path.clone(), all_uses);
                
                // Collect imports for the edge creation pass
                import_map.insert(node_idx, result.imports);
            }
        }
        info!("Symbol indexing complete.");

        // Pass 2.3: Edge Creation
        info!("Creating import edges...");
        for (source_idx, import_paths) in &import_map {
            for target_path in import_paths {
                if let Some(&target_idx) = self.path_to_idx.get(target_path) {
                    if source_idx != &target_idx {
                        self.ensure_edge(*source_idx, target_idx, EdgeKind::Import);
                    }
                } else {
                    // Target doesn't exist - register as unresolved
                    self.unresolved_imports
                        .entry(target_path.clone())
                        .or_default()
                        .insert(*source_idx);
                }
            }
        }
        // Log import resolution summary
        let total_imports: usize = import_map.values().map(|s| s.len()).sum();
        let unresolved: usize = self.unresolved_imports.values().map(|s| s.len()).sum();
        let resolved = total_imports - unresolved;
        info!(
            "Import resolution: {} total, {} resolved to edges, {} unresolved",
            total_imports, resolved, unresolved
        );
        if unresolved > 0 {
            let sample: Vec<_> = self.unresolved_imports.keys().take(5)
                .map(|p| p.display().to_string()).collect();
            info!("Sample unresolved import targets: {:?}", sample);
        }

        self.pagerank_dirty = true;
    }
    
    pub fn add_file(&mut self, path: PathBuf, content: &str) -> Result<(), GraphError> {
        // Canonicalize path to match build() convention
        let path = path.canonicalize().unwrap_or(path);

        // 1. UPSERT PROTECTION
        // If file exists, remove it first to ensure clean state
        if self.path_to_idx.contains_key(&path) {
            self.remove_file(&path)?;
        }
        
        // 2. PARSING (with fallback)
        let lang = SupportedLanguage::from_extension(path.extension().and_then(|s| s.to_str()).unwrap_or(""));
        if lang == SupportedLanguage::Unknown {
            return Ok(()); // Silently skip unsupported files
        }
        
        let parse_result = match parser::parse_with_fallback(content, lang) {
            Ok(res) => res,
            Err(e) => {
                log::error!("Parser initialization failed for {}: {}", path.display(), e);
                return Err(GraphError::ParseError(path));
            }
        };
        
        let tree = match parse_result.tree {
            Some(t) => t,
            None => {
                log::error!("Complete parsing failure for file: {}", path.display());
                return Err(GraphError::ParseError(path));
            }
        };

        if parse_result.has_errors {
            log::warn!("File {} has syntax errors, graph data may be incomplete.", path.display());
        }
        
        let symbols: Vec<Symbol> = self.symbol_harvester.harvest(&tree, content, lang);
        let defs: Vec<String> = symbols.iter().filter(|s| s.is_definition).map(|s| s.name.clone()).collect();
        let uses: Vec<String> = symbols.iter().filter(|s| !s.is_definition).map(|s| s.name.clone()).collect();
        let imports = self.import_resolver.find_imports(&tree, &path, content.as_bytes());
        
        // 3. NODE CREATION
        let file_node = FileNode::new(
            path.clone(),
            lang,
            defs.clone(),
            uses.clone(),
            &imports,
            content,
        );
        
        let new_idx = self.graph.add_node(file_node);
        self.path_to_idx.insert(path.clone(), new_idx);
        
        // 4. SYMBOL INDEX REGISTRATION
        for def in &defs {
            self.symbol_index
                .definitions
                .entry(def.clone())
                .or_default()
                .push(path.clone());
        }
        
        for usage in &uses {
            self.symbol_index
                .users
                .entry(usage.clone())
                .or_default()
                .push(path.clone());
        }
        
        self.symbol_index.usages.insert(path.clone(), uses.clone());
        
        // 5. OUTGOING EDGES (Dependencies)
        for target_path in &imports {
            if let Some(&target_idx) = self.path_to_idx.get(target_path) {
                // Target exists - create edge immediately
                if new_idx != target_idx {
                    self.ensure_edge(new_idx, target_idx, EdgeKind::Import);
                }
            } else {
                // Target doesn't exist yet - register as unresolved
                self.unresolved_imports
                    .entry(target_path.clone())
                    .or_default()
                    .insert(new_idx);
            }
        }
        
        // 6. INCOMING EDGES (Resolution - OPTIMIZED)
        // Check if any nodes were waiting for THIS file
        if let Some(waiting_nodes) = self.unresolved_imports.remove(&path) {
            for waiting_idx in waiting_nodes {
                if waiting_idx != new_idx {
                    self.ensure_edge(waiting_idx, new_idx, EdgeKind::Import);
                }
            }
        }
        
        // 7. SYMBOL USAGE EDGES (import-aware)
        // Only create SymbolUsage edges to files this node imports
        let new_lang = lang;
        let outgoing_import_targets: HashSet<NodeIndex> = self
            .graph
            .edges_directed(new_idx, petgraph::Direction::Outgoing)
            .filter(|e| *e.weight() == EdgeKind::Import)
            .map(|e| e.target())
            .collect();

        for usage in &uses {
            let defining_paths_to_process = if let Some(defining_paths) = self.symbol_index.definitions.get(usage) {
                if defining_paths.len() > MAX_SYMBOL_DEFINITION_FILES { continue; }
                defining_paths.clone()
            } else {
                Vec::new()
            };

            for def_path in defining_paths_to_process {
                if let Some(&def_idx) = self.path_to_idx.get(&def_path) {
                    if new_idx != def_idx && outgoing_import_targets.contains(&def_idx) {
                        let def_lang = self.graph[def_idx].language;
                        if languages_compatible(new_lang, def_lang) {
                            self.ensure_edge(new_idx, def_idx, EdgeKind::SymbolUsage);
                        }
                    }
                }
            }
        }

        // 8. REVERSE SYMBOL USAGE EDGES (import-aware)
        // Only create incoming SymbolUsage edges from files that import this file
        for def in &defs {
            if let Some(def_paths) = self.symbol_index.definitions.get(def) {
                if def_paths.len() > MAX_SYMBOL_DEFINITION_FILES { continue; }
            }
            let using_paths_to_process = if let Some(using_paths) = self.symbol_index.users.get(def) {
                using_paths.clone()
            } else {
                Vec::new()
            };

            for user_path in using_paths_to_process {
                if let Some(&user_idx) = self.path_to_idx.get(&user_path) {
                    if user_idx != new_idx && &user_path != &path {
                        // Check if the user file has an Import edge to this file
                        let has_import = self
                            .graph
                            .edges_directed(user_idx, petgraph::Direction::Outgoing)
                            .any(|e| e.target() == new_idx && *e.weight() == EdgeKind::Import);
                        if has_import {
                            let user_lang = self.graph[user_idx].language;
                            if languages_compatible(user_lang, new_lang) {
                                self.ensure_edge(user_idx, new_idx, EdgeKind::SymbolUsage);
                            }
                        }
                    }
                }
            }
        }
        
        // 9. CPG UPDATE
        if self.cpg.is_some() {
            let cpg_lang = SupportedLanguage::from_path(&path);
            if cpg_lang != SupportedLanguage::Unknown {
                if let Some(ts_lang) = cpg_lang.get_parser() {
                    let mut cpg_parser = TreeSitterParser::new();
                    if cpg_parser.set_language(ts_lang).is_ok() {
                        if let Some(cpg_tree) = cpg_parser.parse(content, None) {
                            if let Some(cpg) = &mut self.cpg {
                                cpg.build_file(&path, cpg_tree, content.to_string(), cpg_lang);
                            }
                        }
                    }
                }
            }
        }

        // 10. FLAG UPDATE
        self.pagerank_dirty = true;

        Ok(())
    }

    pub fn remove_file(&mut self, path: &Path) -> Result<(), GraphError> {
        // Canonicalize path to match build() convention
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let path = canonical.as_path();

        // ===== PHASE 1: PREPARE (Read-Only, Failable) =====

        // 1.1. Resolve path to index
        let target_idx = *self.path_to_idx.get(path)
            .ok_or_else(|| GraphError::NodeNotFound(path.to_path_buf()))?;
        
        // 1.2. Identify swap consequences
        let last_idx = NodeIndex::new(self.graph.node_count() - 1);
        let will_swap = target_idx != last_idx;
        
        // 1.3. Capture the path that will be moved (if swap occurs)
        let moved_path = if will_swap {
            Some(self.graph[last_idx].path.clone())
        } else {
            None
        };
        
        // 1.4. Identify incoming import edges (for downgrading)
        let incoming_importers: Vec<PathBuf> = self.graph
            .edges_directed(target_idx, petgraph::Direction::Incoming)
            .filter(|e| *e.weight() == EdgeKind::Import)
            .map(|e| self.graph[e.source()].path.clone())
            .collect();
        
        // ===== PHASE 2: COMMIT (Infallible Mutations) =====
        
        // 2.1. DOWNGRADE EDGES TO UNRESOLVED
        // Files that imported this file go back to "waiting" state
        for importer_path in incoming_importers {
            if let Some(importer_idx) = self.path_to_idx.get(&importer_path).copied() {
                 self.unresolved_imports
                    .entry(path.to_path_buf())
                    .or_default()
                    .insert(importer_idx);
            }
        }
        
        // 2.2. SYMBOL CLEANUP (Corrected logic from plan)
        let node = self.graph[target_idx].clone(); // Clone to avoid borrow checker issues
        // Remove all definitions this file provided
        for def in &node.definitions {
            if let Some(paths) = self.symbol_index.definitions.get_mut(def) {
                paths.retain(|p| p != path);
                if paths.is_empty() {
                    self.symbol_index.definitions.remove(def);
                }
            }
        }
        
        // Remove all usages this file had
        for r#use in &node.usages {
            if let Some(paths) = self.symbol_index.users.get_mut(r#use) {
                paths.retain(|p| p != path);
                if paths.is_empty() {
                    self.symbol_index.users.remove(r#use);
                }
            }
        }
        
        // Remove this file's own usage list
        self.symbol_index.usages.remove(path);

        // 2.3. GRAPH MUTATION
        // This is where the swap happens!
        self.graph.remove_node(target_idx);
        
        // 2.4. PATH MAP UPDATES
        // Remove the deleted file
        self.path_to_idx.remove(path);
        
        // If a swap occurred, update the map for the moved node
        if let Some(moved_path) = moved_path {
            // The node that WAS at last_idx is NOW at target_idx
            self.path_to_idx.insert(moved_path.clone(), target_idx);
            
            // 2.5. SYMBOL INDEX REMAP
            self.remap_symbol_index(last_idx, target_idx);
            
            // 2.6. UNRESOLVED IMPORTS REMAP
            // If the moved node was waiting for imports, update its index
            for waiting_set in self.unresolved_imports.values_mut() {
                if waiting_set.remove(&last_idx) {
                    waiting_set.insert(target_idx);
                }
            }
        }
        
        // 2.7. CPG CLEANUP
        if let Some(cpg) = &mut self.cpg {
            cpg.remove_file(path);
        }

        // 2.8. FLAG UPDATE
        self.pagerank_dirty = true;

        Ok(())
    }

    /// Helper: Remap all symbol index entries from old_idx to new_idx
    fn remap_symbol_index(&mut self, old_idx: NodeIndex, new_idx: NodeIndex) {
        self.symbol_index.remap_node_index(old_idx, new_idx);
    }

    /// After initial population, this method builds the semantic edges based on symbol usage.
    /// Only creates SymbolUsage edges where an Import edge already exists, ensuring that
    /// symbol matches are backed by actual import relationships.
    pub fn build_semantic_edges(&mut self) {
        info!("Building semantic edges...");

        // Pre-compute import targets: for each node, which other nodes does it import?
        let import_targets: HashMap<NodeIndex, HashSet<NodeIndex>> = self
            .graph
            .node_indices()
            .map(|idx| {
                let targets: HashSet<NodeIndex> = self
                    .graph
                    .edges_directed(idx, petgraph::Direction::Outgoing)
                    .filter(|e| *e.weight() == EdgeKind::Import)
                    .map(|e| e.target())
                    .collect();
                (idx, targets)
            })
            .collect();

        // Pre-compute noisy symbols: skip symbols defined in too many files
        let noisy_symbols: HashSet<&String> = self.symbol_index.definitions.iter()
            .filter(|(_, paths)| paths.len() > MAX_SYMBOL_DEFINITION_FILES)
            .map(|(name, _)| name)
            .collect();
        if !noisy_symbols.is_empty() {
            info!("Filtering {} noisy symbols (defined in >{} files)", noisy_symbols.len(), MAX_SYMBOL_DEFINITION_FILES);
        }

        let edges_to_create: Vec<(NodeIndex, NodeIndex)> = self.symbol_index.usages
            .par_iter()
            .flat_map(|(user_path, used_symbols)| {
                let user_node_idx = match self.path_to_idx.get(user_path) {
                    Some(&idx) => idx,
                    None => return Vec::new(),
                };
                let user_lang = self.graph[user_node_idx].language;

                // Only create SymbolUsage edges to files this node imports
                let user_imports = match import_targets.get(&user_node_idx) {
                    Some(targets) => targets,
                    None => return Vec::new(),
                };
                if user_imports.is_empty() {
                    return Vec::new();
                }

                let mut edges = Vec::new();
                for symbol in used_symbols {
                    if noisy_symbols.contains(symbol) { continue; }
                    if let Some(def_paths) = self.symbol_index.definitions.get(symbol) {
                        for def_path in def_paths {
                            if user_path == def_path { continue; }
                            if let Some(&def_node_idx) = self.path_to_idx.get(def_path) {
                                // Only create edge if the user file imports the defining file
                                if !user_imports.contains(&def_node_idx) { continue; }
                                let def_lang = self.graph[def_node_idx].language;
                                if languages_compatible(user_lang, def_lang) {
                                    edges.push((user_node_idx, def_node_idx));
                                }
                            }
                        }
                    }
                }
                edges
            })
            .collect();

        // Serial mutation of the graph
        for (src, dst) in edges_to_create {
            self.ensure_edge(src, dst, EdgeKind::SymbolUsage);
        }
    }
    
    /// Complete build process: parse files, harvest symbols, build import edges, build semantic edges.
    pub fn build_complete(&mut self, paths: &[PathBuf], _project_root: &Path) {
        // Canonicalize paths once for consistency across build and CPG
        let canonical_paths: Vec<PathBuf> = paths
            .iter()
            .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
            .collect();
        self.build(&canonical_paths);
        self.build_semantic_edges();
        self.calculate_pagerank(20, 0.85);
        info!("Build complete. Final graph: {} nodes, {} edges.", self.graph.node_count(), self.graph.edge_count());

        // CPG overlay: re-parse files to populate sub-file nodes
        if self.cpg.is_some() {
            self.build_cpg_for_all_files(&canonical_paths);
        }

        // Compute import bindings and resolve call graph across all files
        self.compute_all_import_bindings();
        if let Some(cpg) = &mut self.cpg {
            crate::callgraph::CallGraphBuilder::resolve_all(cpg, &self.symbol_index);
        }
    }

    /// Build CPG data for all files (used during build_complete).
    fn build_cpg_for_all_files(&mut self, paths: &[PathBuf]) {
        let total = paths.len();
        let start = Instant::now();
        let mut built = 0usize;

        for (i, path) in paths.iter().enumerate() {
            let lang = SupportedLanguage::from_path(path);
            if lang == SupportedLanguage::Unknown {
                continue;
            }
            let ts_lang = match lang.get_parser() {
                Some(l) => l,
                None => continue,
            };
            let source = match fs::read_to_string(path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let mut parser = TreeSitterParser::new();
            if parser.set_language(ts_lang).is_err() {
                continue;
            }
            let tree = match parser.parse(&source, None) {
                Some(t) => t,
                None => continue,
            };

            let file_start = Instant::now();
            if let Some(cpg) = &mut self.cpg {
                cpg.build_file(path, tree, source, lang);
            }
            let file_elapsed = file_start.elapsed();
            built += 1;

            if file_elapsed.as_secs() > 5 {
                warn!("CPG: slow file ({:.1}s): {}", file_elapsed.as_secs_f64(), path.display());
            }

            if built % 50 == 0 || i == total - 1 {
                info!("CPG progress: {}/{} files built ({:.1}s elapsed)",
                      built, total, start.elapsed().as_secs_f64());
            }
        }

        info!("CPG build complete: {} files in {:.1}s", built, start.elapsed().as_secs_f64());
    }

    /// Compute import bindings for all files in the CPG.
    fn compute_all_import_bindings(&mut self) {
        let cpg = match &self.cpg {
            Some(c) => c,
            None => return,
        };
        // Collect (path, bindings) pairs using &self.import_resolver and &cpg (both shared refs)
        let all_bindings: Vec<(PathBuf, Vec<crate::import_resolver::ImportBinding>)> = cpg
            .trees
            .keys()
            .filter_map(|path| {
                let tree = cpg.trees.get(path)?;
                let source = cpg.sources.get(path)?;
                let bindings = self.import_resolver.find_import_bindings(tree, path, source.as_bytes());
                if bindings.is_empty() {
                    None
                } else {
                    Some((path.clone(), bindings))
                }
            })
            .collect();
        // Now take &mut self.cpg to store them
        if let Some(cpg) = &mut self.cpg {
            for (path, bindings) in all_bindings {
                cpg.import_bindings.insert(path, bindings);
            }
        }
    }

    /// Compute import bindings for a single file in the CPG.
    fn compute_import_bindings_for_file(&mut self, file_path: &Path) {
        let bindings = {
            let cpg = match &self.cpg {
                Some(c) => c,
                None => return,
            };
            let tree = match cpg.trees.get(file_path) {
                Some(t) => t,
                None => return,
            };
            let source = match cpg.sources.get(file_path) {
                Some(s) => s,
                None => return,
            };
            self.import_resolver.find_import_bindings(tree, file_path, source.as_bytes())
        };
        if let Some(cpg) = &mut self.cpg {
            if bindings.is_empty() {
                cpg.import_bindings.remove(file_path);
            } else {
                cpg.import_bindings.insert(file_path.to_path_buf(), bindings);
            }
        }
    }

    /// Get statistics about the graph
    pub fn get_statistics(&self) -> GraphStatistics {
        let (import_edges, symbol_edges) = self.graph.edge_indices().fold((0, 0), |(i, s), edge_idx| {
            match self.graph[edge_idx] {
                EdgeKind::Import => (i + 1, s),
                EdgeKind::SymbolUsage => (i, s + 1),
            }
        });

        GraphStatistics {
            node_count: self.graph.node_count(),
            edge_count: self.graph.edge_count(),
            import_edges,
            symbol_edges,
            total_definitions: self.symbol_index.definitions.len(),
            total_files_with_usages: self.symbol_index.usages.len(),
            unresolved_import_count: self.unresolved_imports.values().map(|s| s.len()).sum(),
            source_roots: self.import_resolver.get_source_roots(),
            module_index_size: self.import_resolver.module_index_size(),
        }
    }

    /// Get details about unresolved imports (for diagnostics).
    pub fn get_unresolved_imports_sample(&self, limit: usize) -> Vec<(PathBuf, usize)> {
        self.unresolved_imports.iter()
            .take(limit)
            .map(|(path, waiters)| (path.clone(), waiters.len()))
            .collect()
    }

    /// Get incoming dependencies for a file
    pub fn get_incoming_dependencies(&self, file_path: &Path) -> Vec<(PathBuf, EdgeKind)> {
        let canonical = file_path.canonicalize().unwrap_or_else(|_| file_path.to_path_buf());
        if let Some(&node_idx) = self.path_to_idx.get(canonical.as_path()) {
            self.graph.edges_directed(node_idx, petgraph::Direction::Incoming)
                .map(|edge| (self.graph[edge.source()].path.clone(), edge.weight().clone()))
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Check if a file path exists in the graph.
    pub fn has_file(&self, file_path: &Path) -> bool {
        let canonical = file_path.canonicalize().unwrap_or_else(|_| file_path.to_path_buf());
        self.path_to_idx.contains_key(canonical.as_path())
    }

    /// Get outgoing dependencies for a file
    pub fn get_outgoing_dependencies(&self, file_path: &Path) -> Vec<(PathBuf, EdgeKind)> {
        let canonical = file_path.canonicalize().unwrap_or_else(|_| file_path.to_path_buf());
        if let Some(&node_idx) = self.path_to_idx.get(canonical.as_path()) {
            self.graph.edges_directed(node_idx, petgraph::Direction::Outgoing)
                .map(|edge| (self.graph[edge.target()].path.clone(), edge.weight().clone()))
                .collect()
        } else {
            Vec::new()
        }
    }

    pub fn calculate_pagerank(&mut self, iterations: usize, damping_factor: f64) {
        let node_count = self.graph.node_count();
        if node_count == 0 { return; }

        // --- OPTIMIZATION: Pre-compute out-degrees ---
        let out_degrees: Vec<f64> = self.graph.node_indices()
            .map(|idx| self.graph.edges_directed(idx, petgraph::Direction::Outgoing).count() as f64)
            .collect();

        let mut ranks: Vec<f64> = vec![1.0 / node_count as f64; node_count];
        
        for _ in 0..iterations {
            let mut new_ranks = vec![0.0; node_count];
            for node_idx in self.graph.node_indices() {
                let mut rank_sum = 0.0;
                for edge in self.graph.edges_directed(node_idx, petgraph::Direction::Incoming) {
                    let source_idx = edge.source();
                    // Use the pre-computed out-degree
                    let source_out_degree = out_degrees[source_idx.index()];
                    if source_out_degree > 0.0 {
                        rank_sum += (ranks[source_idx.index()] / source_out_degree) * edge.weight().strength();
                    }
                }
                new_ranks[node_idx.index()] = (1.0 - damping_factor) / node_count as f64 + damping_factor * rank_sum;
            }
            ranks = new_ranks;
        }

        for (i, rank) in ranks.iter().enumerate() {
            self.graph[NodeIndex::new(i)].rank = *rank;
        }
        self.pagerank_dirty = false;
    }

    pub fn get_top_ranked_files(&mut self, limit: usize) -> Vec<(PathBuf, f64)> {
        self.ensure_pagerank_up_to_date();
        let mut ranked_files: Vec<_> = self.graph.node_weights().map(|node| (node.path.clone(), node.rank)).collect();
        ranked_files.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked_files.truncate(limit);
        ranked_files
    }

    /// Get all files that depend on this file (alias for get_incoming_dependencies)
    pub fn get_dependents(&self, file_path: &Path) -> Vec<(PathBuf, EdgeKind)> {
        self.get_incoming_dependencies(file_path)
    }

    /// Get all files that this file depends on (alias for get_outgoing_dependencies)
    pub fn get_dependencies(&self, file_path: &Path) -> Vec<(PathBuf, EdgeKind)> {
        self.get_outgoing_dependencies(file_path)
    }


    fn calculate_stars(&self, rank: f64, max_rank: f64) -> usize {
        if max_rank == 0.0 { return 1; }
        ((rank / max_rank).clamp(0.0, 1.0) * 5.0).ceil().max(1.0).min(5.0) as usize
    }

    fn format_stars(&self, star_count: usize) -> String { "★".repeat(star_count) }

    fn get_max_rank(&self) -> f64 { self.graph.node_weights().map(|n| n.rank).fold(0.0, f64::max) }

    pub fn generate_map(&mut self, max_files: usize) -> String {
        self.ensure_pagerank_up_to_date();
        
        let mut output = String::new();
        
        writeln!(output, "Repository Map ({} files)\n", self.graph.node_count()).unwrap();
        
        let top_files = self.get_top_ranked_files(max_files); 
        if !top_files.is_empty() {
            writeln!(output, "TOP RANKED FILES (by architectural importance):").unwrap();
            for (i, (path, rank)) in top_files.iter().enumerate() {
                let dependents_count = self.get_dependents(path).len();
                let display_path = pathdiff::diff_paths(path, &self.project_root)
                    .unwrap_or_else(|| path.clone());
                writeln!(output, "  {}. {} [rank: {:.3}, imported by: {} files]", i + 1, display_path.display(), rank, dependents_count).unwrap();
            }
            writeln!(output).unwrap();
        }

        self.generate_directory_structure(&mut output, Some(&self.project_root));
        output
    }

    fn generate_directory_structure(&self, output: &mut String, project_root: Option<&Path>) {
        writeln!(output, "DIRECTORY STRUCTURE:").unwrap();
        let mut all_files: Vec<_> = self.graph.node_weights().map(|n| {
            let path = project_root.and_then(|root| pathdiff::diff_paths(&n.path, root)).unwrap_or_else(|| n.path.clone());
            (path, n.rank)
        }).collect();
        all_files.sort_by(|a, b| a.0.cmp(&b.0));
        let tree = self.build_directory_tree(&all_files);
        let max_rank = self.get_max_rank();
        self.render_directory_structure_recursive(output, &tree, "", "", max_rank);
    }

    fn build_directory_tree(&self, files: &[(PathBuf, f64)]) -> DirTree {
        let mut root = DirTree::new();
        for (path, rank) in files {
            let mut current = &mut root;
            if let Some(parent) = path.parent() {
                for component in parent.components() {
                    if let std::path::Component::Normal(s) = component {
                        current = current.subdirs.entry(s.to_string_lossy().into_owned()).or_insert_with(DirTree::new);
                    }
                }
            }
            current.files.push((path.clone(), *rank));
        }
        root
    }

    fn render_directory_structure_recursive(&self, output: &mut String, tree: &DirTree, prefix: &str, name: &str, max_rank: f64) {
        if !name.is_empty() { writeln!(output, "{}├── {}/", prefix, name).unwrap(); }
        let child_prefix = if name.is_empty() { String::new() } else { format!("{}│   ", prefix) };
        let mut sorted_subdirs: Vec<_> = tree.subdirs.iter().collect();
        sorted_subdirs.sort_by_key(|(name, _)| *name);

        for (i, (subdir_name, subtree)) in sorted_subdirs.iter().enumerate() {
            let is_last = i == sorted_subdirs.len() - 1 && tree.files.is_empty();
            let new_prefix = if is_last { format!("{}    ", prefix) } else { format!("{}│   ", prefix) };
            self.render_directory_structure_recursive(output, subtree, &new_prefix, subdir_name, max_rank);
        }

        let mut sorted_files: Vec<_> = tree.files.iter().collect();
        sorted_files.sort_by_key(|(path, _)| path.file_name());

        for (i, (file_path, rank)) in sorted_files.iter().enumerate() {
            let is_last = i == sorted_files.len() - 1;
            let connector = if is_last { "└──" } else { "├──" };
            let file_name = file_path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown");
            let star_str = self.format_stars(self.calculate_stars(*rank, max_rank));
            writeln!(output, "{}{} {} {} (rank: {:.3})", child_prefix, connector, file_name, star_str, rank).unwrap();
        }
    }

    /// Validates the internal consistency of the symbol index and graph.
    /// Used in tests and can be called after updates in debug mode.
    pub fn validate_consistency(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        // Check 1: Size check
        if self.path_to_idx.len() != self.graph.node_count() {
            errors.push(format!(
                "Size mismatch: path_to_idx has {} entries, graph has {} nodes",
                self.path_to_idx.len(),
                self.graph.node_count()
            ));
        }
        
        // Check 2: Mapping integrity
        for (path, &idx) in &self.path_to_idx {
            if let Some(node) = self.graph.node_weight(idx) {
                if node.path != *path {
                    errors.push(format!(
                        "Mismatch at index {:?}: path_to_idx expects '{}', but node contains '{}'",
                        idx, path.display(), node.path.display()
                    ));
                }
            } else {
                errors.push(format!(
                    "path_to_idx points to invalid index {:?} for path '{}'",
                    idx, path.display()
                ));
            }
        }
        
        // Check 3: Reverse check
        for idx in self.graph.node_indices() {
            let node = &self.graph[idx];
            if !self.path_to_idx.contains_key(&node.path) {
                errors.push(format!(
                    "Node at index {:?} with path '{}' has no path_to_idx entry",
                    idx, node.path.display()
                ));
            }
        }

        // Check 4: Every definition in symbol_index points to a real file node
        for (symbol, def_paths) in &self.symbol_index.definitions {
            for path in def_paths {
                if !self.path_to_idx.contains_key(path) {
                    errors.push(format!(
                        "Symbol '{}' has definition in non-existent file: {}",
                        symbol,
                        path.display()
                    ));
                }
            }
        }

        // Check 5: Every file in symbol_index.usages exists in the graph
        for (path, _) in &self.symbol_index.usages {
            if !self.path_to_idx.contains_key(path) {
                errors.push(format!(
                    "Symbol usage tracked for non-existent file: {}",
                    path.display()
                    ));
            }
        }
        
        
        // Check 6: File node definitions match symbol_index
        for (path, &node_idx) in &self.path_to_idx {
            let node = &self.graph[node_idx];
            
            // Every definition in the node should be in symbol_index
            for def in &node.definitions {
                if let Some(paths) = self.symbol_index.definitions.get(def) {
                    if !paths.contains(path) {
                        errors.push(format!(
                            "File {} defines '{}' but not tracked in symbol_index",
                            path.display(), def
                        ));
                    }
                } else {
                    errors.push(format!(
                            "File {} defines '{}' but not in symbol_index at all",
                            path.display(), def
                        ));
                }
            }
        }
        
        // Check 7: SymbolUsage edges correspond to symbol_index entries
        for edge_idx in self.graph.edge_indices() {
            if self.graph[edge_idx] == EdgeKind::SymbolUsage {
                let (source_idx, target_idx) = self.graph.edge_endpoints(edge_idx).unwrap();
                let source_path = &self.graph[source_idx].path;
                let target_path = &self.graph[target_idx].path;
                
                // Source should have some usages
                if !self.symbol_index.usages.contains_key(source_path) {
                    errors.push(format!(
                        "SymbolUsage edge from {} but no usages tracked",
                        source_path.display()
                    ));
                }
                
                // Target should define something the source uses
                let target_node = &self.graph[target_idx];
                if let Some(source_usages) = self.symbol_index.usages.get(source_path) {
                    let has_matching_symbol = target_node.definitions.iter() 
                        .any(|def| source_usages.contains(def));
                    
                    if !has_matching_symbol {
                        errors.push(format!(
                            "SymbolUsage edge {} -> {} but no matching symbol found",
                            source_path.display(), target_path.display()
                        ));
                    }
                }
            }
        }
        
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}
