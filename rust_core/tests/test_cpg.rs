use semantic_engine::cpg::{CpgLayer, CpgNodeKind};
use semantic_engine::graph::RepoGraph;
use semantic_engine::parser::SupportedLanguage;
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;
use tree_sitter::Parser as TreeSitterParser;

/// Helper: parse Python source and build CPG for it.
fn build_cpg_for_source(cpg: &mut CpgLayer, path: &str, source: &str) {
    let path = PathBuf::from(path);
    let mut parser = TreeSitterParser::new();
    parser
        .set_language(SupportedLanguage::Python.get_parser().unwrap())
        .unwrap();
    let tree = parser.parse(source, None).unwrap();
    cpg.build_file(&path, tree, source.to_string(), SupportedLanguage::Python);
}

fn create_test_file(root: &std::path::Path, path: &str, content: &str) {
    let file_path = root.join(path);
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(file_path, content).unwrap();
}

// ============================================================
// Test 1: Python function with typed params + return type + docstring
// ============================================================
#[test]
fn test_python_function_with_typed_params() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def greet(name: str, count: int = 1) -> str:
    """Say hello multiple times."""
    return (name + "\n") * count
"#;
    build_cpg_for_source(&mut cpg, "/test/main.py", source);

    let path = PathBuf::from("/test/main.py");
    let funcs = cpg.get_functions_in_file(&path);
    assert_eq!(funcs.len(), 1);

    let f = &funcs[0];
    assert_eq!(f.name, "greet");
    assert_eq!(f.kind, CpgNodeKind::Function);
    assert_eq!(f.return_type.as_deref(), Some("str"));
    assert!(f.docstring.as_ref().unwrap().contains("Say hello"));

    assert_eq!(f.parameters.len(), 2);
    assert_eq!(f.parameters[0].name, "name");
    assert_eq!(f.parameters[0].type_annotation.as_deref(), Some("str"));
    assert!(f.parameters[0].default_value.is_none());

    assert_eq!(f.parameters[1].name, "count");
    assert_eq!(f.parameters[1].type_annotation.as_deref(), Some("int"));
    assert_eq!(f.parameters[1].default_value.as_deref(), Some("1"));
}

// ============================================================
// Test 2: Class with bases + methods (AstChild edges)
// ============================================================
#[test]
fn test_class_with_bases_and_methods() {
    let mut cpg = CpgLayer::new();
    let source = r#"
class Dog(Animal, Serializable):
    """A good dog."""
    def bark(self):
        pass

    def fetch(self, item: str) -> bool:
        return True
"#;
    build_cpg_for_source(&mut cpg, "/test/animals.py", source);

    let path = PathBuf::from("/test/animals.py");

    let classes = cpg.get_classes_in_file(&path);
    assert_eq!(classes.len(), 1);
    let cls = &classes[0];
    assert_eq!(cls.name, "Dog");
    assert_eq!(cls.bases, vec!["Animal", "Serializable"]);
    assert!(cls.docstring.as_ref().unwrap().contains("good dog"));

    let funcs = cpg.get_functions_in_file(&path);
    assert_eq!(funcs.len(), 2);
    assert!(funcs.iter().all(|f| f.kind == CpgNodeKind::Method));
    assert!(funcs.iter().all(|f| f.parent_class.as_deref() == Some("Dog")));

    // Verify AstChild edges from class to methods
    let class_idx = cpg.file_to_nodes.get(&path).unwrap().iter().find(|idx| {
        cpg.graph.node_weight(**idx).map(|n| n.kind == CpgNodeKind::Class).unwrap_or(false)
    }).unwrap();
    let children = cpg.get_children(*class_idx);
    assert_eq!(children.len(), 2);
}

// ============================================================
// Test 3: Module-level variable extraction
// ============================================================
#[test]
fn test_module_level_variable() {
    let mut cpg = CpgLayer::new();
    let source = r#"
MAX_RETRIES = 5
API_URL = "https://example.com"
"#;
    build_cpg_for_source(&mut cpg, "/test/config.py", source);

    let path = PathBuf::from("/test/config.py");
    let nodes = cpg.get_nodes_for_file(&path);

    let vars: Vec<_> = nodes.iter().filter(|n| n.kind == CpgNodeKind::Variable).collect();
    assert_eq!(vars.len(), 2);

    let names: Vec<&str> = vars.iter().map(|v| v.name.as_str()).collect();
    assert!(names.contains(&"MAX_RETRIES"));
    assert!(names.contains(&"API_URL"));
}

// ============================================================
// Test 4: Decorated functions/methods
// ============================================================
#[test]
fn test_decorated_function() {
    let mut cpg = CpgLayer::new();
    let source = r#"
@app.route("/")
def index():
    return "hello"

class Service:
    @staticmethod
    def create():
        pass

    @classmethod
    def from_config(cls, config: dict):
        pass
"#;
    build_cpg_for_source(&mut cpg, "/test/app.py", source);

    let path = PathBuf::from("/test/app.py");
    let funcs = cpg.get_functions_in_file(&path);
    assert_eq!(funcs.len(), 3);

    let names: Vec<&str> = funcs.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"index"));
    assert!(names.contains(&"create"));
    assert!(names.contains(&"from_config"));

    let index_fn = funcs.iter().find(|f| f.name == "index").unwrap();
    assert_eq!(index_fn.kind, CpgNodeKind::Function);

    let create_fn = funcs.iter().find(|f| f.name == "create").unwrap();
    assert_eq!(create_fn.kind, CpgNodeKind::Method);
    assert_eq!(create_fn.parent_class.as_deref(), Some("Service"));
}

// ============================================================
// Test 5: Async functions
// ============================================================
#[test]
fn test_async_function() {
    let mut cpg = CpgLayer::new();
    let source = r#"
async def fetch_data(url: str) -> dict:
    """Fetch data from a URL."""
    pass
"#;
    build_cpg_for_source(&mut cpg, "/test/async_mod.py", source);

    let path = PathBuf::from("/test/async_mod.py");
    let funcs = cpg.get_functions_in_file(&path);
    assert_eq!(funcs.len(), 1);

    let f = &funcs[0];
    assert_eq!(f.name, "fetch_data");
    assert_eq!(f.kind, CpgNodeKind::Function);
    assert_eq!(f.return_type.as_deref(), Some("dict"));
    assert!(f.docstring.as_ref().unwrap().contains("Fetch data"));
}

// ============================================================
// Test 6: *args/**kwargs parameters
// ============================================================
#[test]
fn test_args_kwargs_parameters() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def variadic(a, *args, **kwargs):
    pass
"#;
    build_cpg_for_source(&mut cpg, "/test/var.py", source);

    let path = PathBuf::from("/test/var.py");
    let funcs = cpg.get_functions_in_file(&path);
    assert_eq!(funcs.len(), 1);

    let f = &funcs[0];
    assert_eq!(f.parameters.len(), 3);
    assert_eq!(f.parameters[0].name, "a");
    assert_eq!(f.parameters[1].name, "*args");
    assert_eq!(f.parameters[2].name, "**kwargs");
}

// ============================================================
// Test 7: Default parameter values
// ============================================================
#[test]
fn test_default_parameter_values() {
    let mut cpg = CpgLayer::new();
    let source = r#"
def configure(timeout=30, retries=3, verbose=False):
    pass
"#;
    build_cpg_for_source(&mut cpg, "/test/cfg.py", source);

    let path = PathBuf::from("/test/cfg.py");
    let funcs = cpg.get_functions_in_file(&path);
    assert_eq!(funcs.len(), 1);

    let f = &funcs[0];
    assert_eq!(f.parameters.len(), 3);
    assert_eq!(f.parameters[0].default_value.as_deref(), Some("30"));
    assert_eq!(f.parameters[1].default_value.as_deref(), Some("3"));
    assert_eq!(f.parameters[2].default_value.as_deref(), Some("False"));
}

// ============================================================
// Test 8: Integration with RepoGraph (enable_cpg + build_complete)
// ============================================================
#[test]
fn test_repograph_cpg_integration() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "models.py", r#"
class User:
    def __init__(self, name: str):
        self.name = name

    def greet(self) -> str:
        return f"Hello, {self.name}"
"#);

    create_test_file(&root_path, "utils.py", r#"
def format_name(first: str, last: str) -> str:
    return f"{first} {last}"
"#);

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    graph.enable_cpg();

    let paths = vec![
        root_path.join("models.py"),
        root_path.join("utils.py"),
    ];
    graph.build_complete(&paths, &root_path);

    let cpg = graph.cpg.as_ref().unwrap();

    // Check models.py
    let model_funcs = cpg.get_functions_in_file(&root_path.join("models.py"));
    assert_eq!(model_funcs.len(), 2);

    let model_classes = cpg.get_classes_in_file(&root_path.join("models.py"));
    assert_eq!(model_classes.len(), 1);
    assert_eq!(model_classes[0].name, "User");

    // Check utils.py
    let util_funcs = cpg.get_functions_in_file(&root_path.join("utils.py"));
    assert_eq!(util_funcs.len(), 1);
    assert_eq!(util_funcs[0].name, "format_name");
}

// ============================================================
// Test 9: Incremental update (modify file -> CPG nodes refresh)
// ============================================================
#[test]
fn test_cpg_incremental_update() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    let original = r#"
def hello():
    pass
"#;
    create_test_file(&root_path, "mod.py", original);

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    graph.enable_cpg();
    let paths = vec![root_path.join("mod.py")];
    graph.build_complete(&paths, &root_path);

    // Verify initial state
    let cpg = graph.cpg.as_ref().unwrap();
    assert_eq!(cpg.get_functions_in_file(&root_path.join("mod.py")).len(), 1);

    // Now update with new content
    let updated = r#"
def hello():
    pass

def goodbye(name: str) -> str:
    return f"bye {name}"
"#;
    fs::write(root_path.join("mod.py"), updated).unwrap();
    let mod_path = root_path.join("mod.py");
    graph.update_file(&mod_path, updated).unwrap();

    let cpg = graph.cpg.as_ref().unwrap();
    let funcs = cpg.get_functions_in_file(&root_path.join("mod.py"));
    assert_eq!(funcs.len(), 2);

    let names: Vec<&str> = funcs.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"hello"));
    assert!(names.contains(&"goodbye"));
}

// ============================================================
// Test 10: File removal (CPG nodes cleaned up, other files intact)
// ============================================================
#[test]
fn test_cpg_file_removal() {
    let root = tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();

    create_test_file(&root_path, "a.py", "def func_a():\n    pass\n");
    create_test_file(&root_path, "b.py", "def func_b():\n    pass\n");

    let mut graph = RepoGraph::new(&root_path, "python", &[], None);
    graph.enable_cpg();
    let paths = vec![root_path.join("a.py"), root_path.join("b.py")];
    graph.build_complete(&paths, &root_path);

    // Both files should have CPG nodes
    let cpg = graph.cpg.as_ref().unwrap();
    assert_eq!(cpg.get_functions_in_file(&root_path.join("a.py")).len(), 1);
    assert_eq!(cpg.get_functions_in_file(&root_path.join("b.py")).len(), 1);

    // Remove a.py
    graph.remove_file(&root_path.join("a.py")).unwrap();

    let cpg = graph.cpg.as_ref().unwrap();
    assert_eq!(cpg.get_functions_in_file(&root_path.join("a.py")).len(), 0);
    // b.py should still be intact
    assert_eq!(cpg.get_functions_in_file(&root_path.join("b.py")).len(), 1);
}

// ============================================================
// Test 11: AST persistence (get_tree/get_source)
// ============================================================
#[test]
fn test_ast_persistence() {
    let mut cpg = CpgLayer::new();
    let source = "def hello():\n    pass\n";
    build_cpg_for_source(&mut cpg, "/test/hello.py", source);

    let path = PathBuf::from("/test/hello.py");

    // Tree should be persisted
    let tree = cpg.get_tree(&path);
    assert!(tree.is_some());
    let root = tree.unwrap().root_node();
    assert_eq!(root.kind(), "module");

    // Source should be persisted
    let stored_source = cpg.get_source(&path);
    assert_eq!(stored_source, Some(source));
}

// ============================================================
// Test 12: Empty file -> no CPG nodes
// ============================================================
#[test]
fn test_empty_file() {
    let mut cpg = CpgLayer::new();
    build_cpg_for_source(&mut cpg, "/test/empty.py", "");

    let path = PathBuf::from("/test/empty.py");
    assert!(cpg.get_nodes_for_file(&path).is_empty());
    // But tree and source should still be stored
    assert!(cpg.get_tree(&path).is_some());
    assert_eq!(cpg.get_source(&path), Some(""));
}

// ============================================================
// Test 13: Unsupported language -> no CPG nodes
// ============================================================
#[test]
fn test_unsupported_language() {
    let mut cpg = CpgLayer::new();
    let path = PathBuf::from("/test/main.rs");
    let source = "fn main() {}";

    let mut parser = TreeSitterParser::new();
    parser
        .set_language(SupportedLanguage::Rust.get_parser().unwrap())
        .unwrap();
    let tree = parser.parse(source, None).unwrap();
    cpg.build_file(&path, tree, source.to_string(), SupportedLanguage::Rust);

    // No CPG nodes for Rust (deferred)
    assert!(cpg.get_nodes_for_file(&path).is_empty());
    // But tree and source should still be stored
    assert!(cpg.get_tree(&path).is_some());
}

// ============================================================
// Test 14: Multiple files in CpgLayer
// ============================================================
#[test]
fn test_multiple_files() {
    let mut cpg = CpgLayer::new();

    build_cpg_for_source(&mut cpg, "/test/a.py", "def func_a():\n    pass\n");
    build_cpg_for_source(&mut cpg, "/test/b.py", "def func_b():\n    pass\ndef func_c():\n    pass\n");
    build_cpg_for_source(&mut cpg, "/test/c.py", "X = 42\n");

    let a = PathBuf::from("/test/a.py");
    let b = PathBuf::from("/test/b.py");
    let c = PathBuf::from("/test/c.py");

    assert_eq!(cpg.get_functions_in_file(&a).len(), 1);
    assert_eq!(cpg.get_functions_in_file(&b).len(), 2);
    assert_eq!(cpg.get_nodes_for_file(&c).len(), 1);

    // Count only top-level symbol nodes (Function/Method/Class/Variable), not CFG nodes
    let symbol_count = cpg.graph.node_weights()
        .filter(|n| matches!(n.kind, CpgNodeKind::Function | CpgNodeKind::Method | CpgNodeKind::Class | CpgNodeKind::Variable))
        .count();
    assert_eq!(symbol_count, 4); // 1 + 2 + 1
}

// ============================================================
// Test 15: Complex class hierarchy (nested decorated methods)
// ============================================================
#[test]
fn test_complex_class_hierarchy() {
    let mut cpg = CpgLayer::new();
    let source = r#"
class Base:
    def base_method(self):
        pass

class Child(Base):
    """A child class with decorated methods."""

    @property
    def name(self) -> str:
        return self._name

    @name.setter
    def name(self, value: str):
        self._name = value

    @staticmethod
    def create(data: dict) -> "Child":
        pass

    async def async_method(self, url: str) -> dict:
        pass
"#;
    build_cpg_for_source(&mut cpg, "/test/hierarchy.py", source);

    let path = PathBuf::from("/test/hierarchy.py");

    let classes = cpg.get_classes_in_file(&path);
    assert_eq!(classes.len(), 2);

    let base = classes.iter().find(|c| c.name == "Base").unwrap();
    assert!(base.bases.is_empty());

    let child = classes.iter().find(|c| c.name == "Child").unwrap();
    assert_eq!(child.bases, vec!["Base"]);
    assert!(child.docstring.as_ref().unwrap().contains("child class"));

    let all_funcs = cpg.get_functions_in_file(&path);

    // Base has 1 method, Child has 4 methods (name getter, name setter, create, async_method)
    let base_methods: Vec<_> = all_funcs.iter().filter(|f| f.parent_class.as_deref() == Some("Base")).collect();
    assert_eq!(base_methods.len(), 1);

    let child_methods: Vec<_> = all_funcs.iter().filter(|f| f.parent_class.as_deref() == Some("Child")).collect();
    assert_eq!(child_methods.len(), 4);

    // Verify async method details
    let async_m = child_methods.iter().find(|f| f.name == "async_method").unwrap();
    assert_eq!(async_m.return_type.as_deref(), Some("dict"));
    assert_eq!(async_m.parameters.len(), 2); // self, url

    // Verify static method
    let static_m = child_methods.iter().find(|f| f.name == "create").unwrap();
    assert_eq!(static_m.parameters.len(), 1); // data
    assert_eq!(static_m.parameters[0].name, "data");
}
