use semantic_engine::call_index::CallIndex;
use semantic_engine::import_resolver::ImportBinding;
use semantic_engine::parser::SupportedLanguage;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tree_sitter::Parser;

fn update(index: &mut CallIndex, path: &str, source: &str, imports: Vec<ImportBinding>) {
    let mut parser = Parser::new();
    parser
        .set_language(SupportedLanguage::Python.get_parser().unwrap())
        .unwrap();
    let tree = parser.parse(source, None).unwrap();
    assert!(!tree.root_node().has_error());
    index.update_file(
        Path::new(path),
        &tree,
        source,
        SupportedLanguage::Python,
        imports,
    );
}
fn import(local: &str, path: &str, symbol: Option<&str>) -> ImportBinding {
    ImportBinding {
        local_name: local.into(),
        resolved_path: PathBuf::from(path),
        imported_symbol: symbol.map(String::from),
    }
}
fn targets(index: &CallIndex, path: &str, function: &str) -> Vec<String> {
    let f = index.find_functions(Path::new(path), function);
    assert_eq!(f.len(), 1, "{function}");
    index
        .callees(&f[0].id)
        .iter()
        .map(|f| format!("{}:{}", f.file_path.display(), f.qualified_name))
        .collect()
}
fn edges(index: &CallIndex, paths: &[&str]) -> BTreeSet<(String, String)> {
    paths
        .iter()
        .flat_map(|path| index.functions(Path::new(path)))
        .flat_map(|f| {
            index
                .callees(&f.id)
                .iter()
                .map(|callee| {
                    (
                        format!("{}:{}", f.file_path.display(), f.qualified_name),
                        format!("{}:{}", callee.file_path.display(), callee.qualified_name),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect()
}
#[test]
fn lexical_scope_shadowing_and_qualified_candidates() {
    let mut index = CallIndex::new();
    update(
        &mut index,
        "/r/a.py",
        r#"def process(): pass
class A:
    def process(self): pass
class B:
    def process(self): pass
def param(process): process()
def assigned():
    process()
    process = unknown
def unknown_receiver(obj): obj.process()
def run():
    a = A()
    a.process()
    process()
def outer():
    def process(): pass
    def inner(): process()
    inner()
"#,
        vec![],
    );
    index.resolve_all();
    for name in ["param", "assigned", "unknown_receiver"] {
        assert!(targets(&index, "/r/a.py", name).is_empty());
    }
    assert_eq!(
        targets(&index, "/r/a.py", "run"),
        vec!["/r/a.py:A.process", "/r/a.py:process"]
    );
    assert_eq!(
        targets(&index, "/r/a.py", "outer.inner"),
        vec!["/r/a.py:outer.process"]
    );
    assert_eq!(
        targets(&index, "/r/a.py", "outer"),
        vec!["/r/a.py:outer.inner"]
    );
    assert_eq!(
        index.find_functions(Path::new("/r/a.py"), "process").len(),
        4
    );
}
#[test]
fn aliases_reexports_and_import_binding_changes() {
    let mut index = CallIndex::new();
    update(&mut index, "/r/a.py", "def process(): pass\n", vec![]);
    update(&mut index, "/r/b.py", "def process(): pass\n", vec![]);
    update(
        &mut index,
        "/r/api.py",
        "from a import process as exported\n",
        vec![import("exported", "/r/a.py", Some("process"))],
    );
    update(&mut index,"/r/main.py","from api import exported as fn\nimport api as module\ndef run():\n    fn()\n    module.exported()\n",vec![import("fn","/r/api.py",Some("exported")),import("module","/r/api.py",None)]);
    index.resolve_all();
    assert_eq!(
        targets(&index, "/r/main.py", "run"),
        vec!["/r/a.py:process"]
    );
    update(
        &mut index,
        "/r/api.py",
        "from b import process as exported\n",
        vec![import("exported", "/r/b.py", Some("process"))],
    );
    index.resolve_changed(&["/r/api.py".into()]);
    assert_eq!(
        targets(&index, "/r/main.py", "run"),
        vec!["/r/b.py:process"]
    );
}
#[test]
fn unresolved_import_create_delete_and_recreate() {
    let mut index = CallIndex::new();
    update(
        &mut index,
        "/r/main.py",
        "from future import helper\ndef run(): helper()\n",
        vec![],
    );
    index.resolve_all();
    assert!(targets(&index, "/r/main.py", "run").is_empty());
    assert!(index
        .affected_files(&["/r/future.py".into()])
        .contains(&"/r/main.py".into()));
    update(&mut index, "/r/future.py", "def helper(): pass\n", vec![]);
    index.set_import_bindings(
        Path::new("/r/main.py"),
        vec![import("helper", "/r/future.py", Some("helper"))],
    );
    index.resolve_changed(&["/r/future.py".into()]);
    assert_eq!(
        targets(&index, "/r/main.py", "run"),
        vec!["/r/future.py:helper"]
    );
    index.remove_file(Path::new("/r/future.py"));
    index.set_import_bindings(Path::new("/r/main.py"), vec![]);
    index.resolve_changed(&["/r/future.py".into()]);
    assert!(targets(&index, "/r/main.py", "run").is_empty());
    update(
        &mut index,
        "/r/future.py",
        "def helper(): return 2\n",
        vec![],
    );
    index.set_import_bindings(
        Path::new("/r/main.py"),
        vec![import("helper", "/r/future.py", Some("helper"))],
    );
    index.resolve_changed(&["/r/future.py".into()]);
    assert_eq!(
        targets(&index, "/r/main.py", "run"),
        vec!["/r/future.py:helper"]
    );
}
#[test]
fn incremental_matches_fresh_for_edit_sequence() {
    let mut index = CallIndex::new();
    let paths = ["/r/a.py", "/r/b.py", "/r/main.py"];
    let main="from a import first\nfrom b import second\ndef caller():\n    first()\n    second()\ndef outer(): caller()\n";
    let imports = vec![
        import("first", paths[0], Some("first")),
        import("second", paths[1], Some("second")),
    ];
    update(&mut index, paths[0], "def first(): return 0\n", vec![]);
    update(&mut index, paths[1], "def second(): pass\n", vec![]);
    update(&mut index, paths[2], main, imports.clone());
    index.resolve_all();
    for source in [
        "def first(): return 1\n",
        "def renamed(): pass\n",
        "def first(): pass\ndef extra(): pass\n",
        "def first(): return 4\n",
    ] {
        update(&mut index, paths[0], source, vec![]);
        index.resolve_changed(&[paths[0].into()]);
        let mut fresh = CallIndex::new();
        update(&mut fresh, paths[2], main, imports.clone());
        update(&mut fresh, paths[1], "def second(): pass\n", vec![]);
        update(&mut fresh, paths[0], source, vec![]);
        fresh.resolve_all();
        assert_eq!(edges(&index, &paths), edges(&fresh, &paths));
        assert!(targets(&index, paths[2], "caller").contains(&"/r/b.py:second".into()));
        let caller = index
            .find_functions(Path::new(paths[2]), "caller")
            .pop()
            .unwrap();
        assert_eq!(index.callers(&caller.id)[0].name, "outer");
    }
}
#[test]
fn conditional_receiver_and_reassigned_instance_are_unresolved() {
    let mut index = CallIndex::new();
    update(&mut index,"/r/a.py","class A:\n    def process(self): pass\ndef branch(flag):\n    if flag: obj = A()\n    obj.process()\ndef changed():\n    obj = A()\n    obj = unknown\n    obj.process()\n",vec![]);
    index.resolve_all();
    assert!(targets(&index, "/r/a.py", "branch").is_empty());
    assert!(targets(&index, "/r/a.py", "changed").is_empty());
}
#[test]
fn builtin_name_override_is_lexically_resolved() {
    let mut index = CallIndex::new();
    update(
        &mut index,
        "/r/a.py",
        "def print(x): pass\ndef run(): print(1)\n",
        vec![],
    );
    index.resolve_all();
    assert_eq!(targets(&index, "/r/a.py", "run"), vec!["/r/a.py:print"]);
}
#[test]
fn cyclic_reexport_is_bounded() {
    let mut index = CallIndex::new();
    update(
        &mut index,
        "/r/a.py",
        "from b import fn\ndef run(): fn()\n",
        vec![import("fn", "/r/b.py", Some("fn"))],
    );
    update(
        &mut index,
        "/r/b.py",
        "from a import fn\n",
        vec![import("fn", "/r/a.py", Some("fn"))],
    );
    index.resolve_all();
    assert!(targets(&index, "/r/a.py", "run").is_empty());
}

#[test]
fn context_exception_and_pattern_bindings_shadow_globals() {
    let cases = [
        "def run():\n    with context() as process:\n        process()\n",
        "def run():\n    try: fail()\n    except Error as process:\n        process()\n",
        "def run(value):\n    match value:\n        case process:\n            process()\n",
        "def run():\n    for process in values:\n        process()\n",
        "def run():\n    (process, other) = values\n    process()\n",
        "def run(*process):\n    process()\n",
        "def run(**process):\n    process()\n",
    ];
    let mut errors = Vec::new();
    for body in cases {
        let source = format!("def process(): pass\n{body}");
        let mut index = CallIndex::new();
        update(&mut index, "/r/a.py", &source, vec![]);
        index.resolve_all();
        if !targets(&index, "/r/a.py", "run").is_empty() {
            let mut parser = Parser::new();
            parser
                .set_language(SupportedLanguage::Python.get_parser().unwrap())
                .unwrap();
            errors.push(format!(
                "Shadowed call resolved: {body}\nAST: {}",
                parser.parse(&source, None).unwrap().root_node().to_sexp()
            ));
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[test]
fn staticmethod_receiver_name_does_not_prove_a_class() {
    let mut index = CallIndex::new();
    update(&mut index,"/r/a.py","class A:\n    def process(self): pass\n    @staticmethod\n    def run(self): self.process()\n",vec![]);
    index.resolve_all();
    assert!(targets(&index, "/r/a.py", "A.run").is_empty());
}

#[test]
fn repograph_new_import_target_refreshes_existing_importer_without_cpg() {
    use semantic_engine::graph::RepoGraph;
    use semantic_engine::import_resolver::ResolverGroup;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let caller = root.join("main.py");
    let target = root.join("future.py");
    let caller_source = "from future import helper\ndef run(): helper()\n";
    std::fs::write(&caller, caller_source).unwrap();
    let mut graph = RepoGraph::new_multi(&root, &[ResolverGroup::Python], &[], None);
    graph.build_complete(&[caller.clone()], &root);
    assert!(
        graph.cpg.is_none(),
        "Call indexing must not eagerly create detailed CFG/dataflow"
    );
    let run = graph
        .call_index
        .find_functions(&caller, "run")
        .pop()
        .unwrap();
    assert!(graph.call_index.callees(&run.id).is_empty());
    let target_source = "def helper(): return 1\n";
    std::fs::write(&target, target_source).unwrap();
    graph.add_file(target.clone(), target_source).unwrap();
    let callees = graph.call_index.callees(&run.id);
    assert_eq!(
        callees.len(),
        1,
        "Previously unresolved import must bind when file appears"
    );
    assert_eq!(callees[0].name, "helper");
    assert!(graph.cpg.is_none());
    std::fs::remove_file(&target).unwrap();
    graph.remove_file(&target).unwrap();
    assert!(graph.call_index.callees(&run.id).is_empty());
    std::fs::write(&target, target_source).unwrap();
    graph.add_file(target.clone(), target_source).unwrap();
    assert_eq!(graph.call_index.callees(&run.id).len(), 1);
}

#[test]
fn file_order_and_repeated_query_resolution_are_identical() {
    let paths = ["/r/main.py", "/r/a.py", "/r/b.py"];
    let sources=["from a import A\ndef run():\n    value = A()\n    value.process()\ndef unknown(obj): obj.process()\n","class A:\n    def process(self): pass\n","class B:\n    def process(self): pass\n"];
    let mut expected = None;
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let mut index = CallIndex::new();
        for i in order {
            update(
                &mut index,
                paths[i],
                sources[i],
                if i == 0 {
                    vec![import("A", paths[1], Some("A"))]
                } else {
                    vec![]
                },
            );
            index.resolve_changed(&[paths[i].into()]);
        }
        let actual = edges(&index, &paths);
        if let Some(expected) = &expected {
            assert_eq!(&actual, expected);
        } else {
            expected = Some(actual.clone());
        }
        index.resolve_all();
        assert_eq!(actual, edges(&index, &paths));
        assert_eq!(targets(&index, paths[0], "run"), vec!["/r/a.py:A.process"]);
        assert!(targets(&index, paths[0], "unknown").is_empty());
    }
}

#[test]
fn detailed_cpg_eviction_cannot_change_lightweight_call_answers() {
    use semantic_engine::graph::RepoGraph;
    use semantic_engine::import_resolver::ResolverGroup;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = root.join("a.py");
    std::fs::write(&path, "def helper(): pass\ndef run(): helper()\n").unwrap();
    let mut graph = RepoGraph::new_multi(&root, &[ResolverGroup::Python], &[], None);
    graph.build_complete(&[path.clone()], &root);
    let run = graph.call_index.find_functions(&path, "run").pop().unwrap();
    let before = graph.call_index.callees(&run.id);
    assert_eq!(before.len(), 1);
    assert!(graph.cpg.is_none());
    assert!(graph.ensure_cpg_for_file(&path));
    graph.cpg.as_mut().unwrap().remove_file(&path);
    assert_eq!(before, graph.call_index.callees(&run.id));
    assert!(graph.ensure_cpg_for_file(&path));
    assert_eq!(before, graph.call_index.callees(&run.id));
}

#[test]
fn import_usage_requires_lexical_binding_and_counts_constants_and_types() {
    let mut index = CallIndex::new();
    update(&mut index, "/r/b.py", "def process(): pass\n", vec![]);
    update(&mut index, "/r/types.py", "class Item: pass\n", vec![]);
    update(&mut index, "/r/constants.py", "LIMIT = 3\n", vec![]);
    let imports = vec![
        import("process", "/r/b.py", Some("process")),
        import("Item", "/r/types.py", Some("Item")),
        import("LIMIT", "/r/constants.py", Some("LIMIT")),
    ];
    update(
        &mut index,
        "/r/shadow.py",
        "from b import process\ndef run(process): return process()\n",
        imports.clone(),
    );
    assert!(index
        .used_import_paths(Path::new("/r/shadow.py"))
        .is_empty());
    update(
        &mut index,
        "/r/attribute.py",
        "from b import process\ndef run(obj): return obj.process()\n",
        imports.clone(),
    );
    assert!(index
        .used_import_paths(Path::new("/r/attribute.py"))
        .is_empty());
    update(&mut index,"/r/consumer.py","from types import Item\nfrom constants import LIMIT\ndef run(x: Item) -> Item:\n    return LIMIT\n",imports.clone());
    assert_eq!(
        index.used_import_paths(Path::new("/r/consumer.py")),
        BTreeSet::from(["/r/types.py".into(), "/r/constants.py".into()])
    );
}

#[test]
fn removed_and_missing_exports_do_not_count_as_semantic_usage() {
    let mut index = CallIndex::new();
    update(&mut index, "/r/b.py", "used = 3\nunused = 4\n", vec![]);
    update(
        &mut index,
        "/r/a.py",
        "from b import used, unused\ndef run(): return used\n",
        vec![
            import("used", "/r/b.py", Some("used")),
            import("unused", "/r/b.py", Some("unused")),
        ],
    );
    assert_eq!(
        index.used_import_paths(Path::new("/r/a.py")),
        BTreeSet::from(["/r/b.py".into()])
    );
    update(&mut index, "/r/b.py", "unused = 4\n", vec![]);
    assert!(index.used_import_paths(Path::new("/r/a.py")).is_empty());
    update(
        &mut index,
        "/r/api.py",
        "from b import used as public\n",
        vec![import("public", "/r/b.py", Some("used"))],
    );
    update(
        &mut index,
        "/r/c.py",
        "from api import public\ndef run(): return public\n",
        vec![import("public", "/r/api.py", Some("public"))],
    );
    assert!(index.used_import_paths(Path::new("/r/c.py")).is_empty());
}

#[test]
fn cpg_file_lru_is_bounded_and_preserves_lightweight_answers() {
    use semantic_engine::cpg::CpgNodeKind;
    use semantic_engine::graph::RepoGraph;
    use semantic_engine::import_resolver::ResolverGroup;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths: Vec<_> = ["a.py", "b.py", "c.py"]
        .iter()
        .map(|name| root.join(name))
        .collect();
    for (i, path) in paths.iter().enumerate() {
        std::fs::write(path,format!("def helper():\n    return {i}\ndef run():\n    value = helper()\n    return value\n")).unwrap();
    }
    let mut graph = RepoGraph::new_multi(&root, &[ResolverGroup::Python], &[], None);
    graph.build_complete(&paths, &root);
    let run = graph
        .call_index
        .find_functions(&paths[0], "run")
        .pop()
        .unwrap();
    let expected = graph.call_index.callees(&run.id);
    graph.enable_cpg();
    graph.cpg.as_mut().unwrap().set_file_capacity(2).unwrap();
    assert!(graph.cpg.as_mut().unwrap().set_file_capacity(0).is_err());
    for order in [[0, 1, 0, 2], [2, 1, 2, 0], [1, 0, 1, 2]] {
        for i in order {
            assert!(graph.ensure_cpg_for_file(&paths[i]));
            assert!(graph.cpg.as_ref().unwrap().file_to_nodes.len() <= 2);
        }
        let cpg = graph.cpg.as_ref().unwrap();
        assert!(cpg.has_file(&paths[order[2]]));
        assert!(cpg.has_file(&paths[order[3]]));
        assert!(!cpg.has_file(&paths[order[1]]));
        assert_eq!(expected, graph.call_index.callees(&run.id));
        for node in cpg.graph.node_weights() {
            if let Some(owner) = node.function_idx {
                let function = cpg
                    .graph
                    .node_weight(owner)
                    .expect("Every statement owner survives swap-remove");
                assert!(matches!(
                    function.kind,
                    CpgNodeKind::Function | CpgNodeKind::Method
                ));
                assert_eq!(node.file_path, function.file_path);
            }
        }
        for function in cpg.graph.node_indices().filter(|&idx| {
            matches!(
                cpg.graph[idx].kind,
                CpgNodeKind::Function | CpgNodeKind::Method
            )
        }) {
            assert!(!cpg.get_cfg_edges_for_function(function).is_empty());
        }
    }
    graph.cpg.as_mut().unwrap().set_file_capacity(1).unwrap();
    assert_eq!(graph.cpg.as_ref().unwrap().file_to_nodes.len(), 1);
}

#[test]
fn bare_dotted_module_and_aliased_module_resolve_without_namespace_guessing() {
    let mut index = CallIndex::new();
    update(&mut index, "/r/pkg/worker.py", "def work(): pass\n", vec![]);
    update(&mut index,"/r/main.py","import pkg.worker\nimport pkg.worker as w\ndef run():\n    pkg.worker.work()\n    w.work()\ndef wrong():\n    pkg.other.work()\n",vec![import("pkg","/r/pkg/worker.py",None),import("w","/r/pkg/worker.py",None)]);
    index.resolve_all();
    assert_eq!(
        targets(&index, "/r/main.py", "run"),
        vec!["/r/pkg/worker.py:work"]
    );
    assert!(targets(&index, "/r/main.py", "wrong").is_empty());
}
