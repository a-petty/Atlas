use semantic_engine::graph::RepoGraph;
use semantic_engine::import_resolver::ResolverGroup;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf, RepoGraph) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("package.json"), "{}").unwrap();
    let mut paths = Vec::new();
    for (name, source) in files {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, source).unwrap();
        paths.push(path);
    }
    let mut graph = RepoGraph::new_multi(&root, &[ResolverGroup::JsTs], &[], None);
    graph.build_complete(&paths, &root);
    (dir, root, graph)
}
fn callees(graph: &RepoGraph, root: &Path, file: &str, name: &str) -> BTreeSet<String> {
    let funcs = graph.call_index.find_functions(&root.join(file), name);
    assert_eq!(funcs.len(), 1, "{file}:{name}");
    graph
        .call_index
        .callees(&funcs[0].id)
        .iter()
        .map(|f| {
            format!(
                "{}:{}",
                f.file_path.strip_prefix(root).unwrap().display(),
                f.qualified_name
            )
        })
        .collect()
}
fn expected(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|s| s.to_string()).collect()
}

#[test]
fn es_named_default_namespace_and_private_exports() {
    let (_dir,root,graph)=repo(&[
        ("lib.js","export function helper() {}\nexport default function primary() {}\nfunction privateFn() {}\n"),
        ("main.js","import main, {helper as h, privateFn} from './lib';\nimport * as ns from './lib';\nexport function run(){ main(); h(); ns.helper(); privateFn(); ns.privateFn(); }\n")]);
    assert_eq!(
        callees(&graph, &root, "main.js", "run"),
        expected(&["lib.js:helper", "lib.js:primary"])
    );
    assert!(graph.cpg.is_none());
}
#[test]
fn es_named_namespace_and_wildcard_reexports() {
    let (_dir,root,graph)=repo(&[
        ("lib.js","export function helper() {}"),
        ("api.js","export {helper as exposed} from './lib'; export * as ns from './lib'; export * from './lib';"),
        ("main.js","import {exposed,ns,helper} from './api'; function run(){exposed();ns.helper();helper();}")]);
    assert_eq!(
        callees(&graph, &root, "main.js", "run"),
        expected(&["lib.js:helper"])
    );
}
#[test]
fn local_export_alias_and_anonymous_default() {
    for default in ["export default function(){ }", "export default () => 3;"] {
        let (_dir,root,graph)=repo(&[
            ("a.js","function helper() {} export {helper as renamed};"),
            ("b.js",default),
            ("main.js","import fn from './b'; import {renamed} from './a'; function run(){fn();renamed();}")]);
        assert_eq!(
            callees(&graph, &root, "main.js", "run"),
            expected(&["a.js:helper", "b.js:default"])
        );
    }
}
#[test]
fn commonjs_static_requires_and_exports() {
    let (_dir,root,graph)=repo(&[
        ("lib.cjs","function helper(){} exports.helper = helper;"),
        ("callable.cjs","function primary(){} module.exports = primary;"),
        ("main.cjs","const ns = require('./lib'); const {helper: h} = require('./lib'); const fn = require('./lib').helper; const direct = require('./callable'); function run(){ns.helper();h();fn();direct();}")]);
    assert_eq!(
        callees(&graph, &root, "main.cjs", "run"),
        expected(&["callable.cjs:primary", "lib.cjs:helper"])
    );
}
#[test]
fn shadowed_require_and_import_parameter_do_not_invent_calls() {
    let (_dir,root,graph)=repo(&[
        ("lib.js","export function helper(){}"),
        ("main.js","import {helper} from './lib'; function run(helper){helper();} function load(require){const ns = require('./lib');ns.helper();}")]);
    assert!(callees(&graph, &root, "main.js", "run").is_empty());
    assert!(callees(&graph, &root, "main.js", "load").is_empty());
}
#[test]
fn type_only_imports_count_as_types_and_never_as_calls() {
    let (_dir,root,graph)=repo(&[
        ("lib.ts","export interface Item {id: number}; export function helper(){}"),
        ("main.ts","import type {Item} from './lib'; import {type helper} from './lib'; function run(x: Item): Item {helper();return x;}")]);
    assert!(callees(&graph, &root, "main.ts", "run").is_empty());
    assert_eq!(
        graph.call_index.used_import_paths(&root.join("main.ts")),
        BTreeSet::from([root.join("lib.ts")])
    );
}
#[test]
fn javascript_reexport_update_and_unresolved_creation_are_fresh() {
    let (_dir, root, mut graph) = repo(&[
        ("a.js", "export function helper(){}"),
        ("b.js", "export function helper(){}"),
        ("api.js", "export {helper} from './a';"),
        (
            "main.js",
            "import {helper} from './api'; function run(){helper();}",
        ),
    ]);
    assert_eq!(
        callees(&graph, &root, "main.js", "run"),
        expected(&["a.js:helper"])
    );
    let changed = "export {helper} from './b';";
    std::fs::write(root.join("api.js"), changed).unwrap();
    graph.update_file(&root.join("api.js"), changed).unwrap();
    assert_eq!(
        callees(&graph, &root, "main.js", "run"),
        expected(&["b.js:helper"])
    );
    let changed = "export {helper} from './future';";
    std::fs::write(root.join("api.js"), changed).unwrap();
    graph.update_file(&root.join("api.js"), changed).unwrap();
    assert!(callees(&graph, &root, "main.js", "run").is_empty());
    let source = "export function helper(){}";
    std::fs::write(root.join("future.js"), source).unwrap();
    graph.add_file(root.join("future.js"), source).unwrap();
    assert_eq!(
        callees(&graph, &root, "main.js", "run"),
        expected(&["future.js:helper"])
    );
}
#[test]
fn conflicting_wildcard_reexports_stay_unresolved() {
    let (_dir, root, graph) = repo(&[
        ("a.js", "export function helper(){}"),
        ("b.js", "export function helper(){}"),
        ("api.js", "export * from './a'; export * from './b';"),
        (
            "main.js",
            "import {helper} from './api'; function run(){helper();}",
        ),
    ]);
    assert!(callees(&graph, &root, "main.js", "run").is_empty());
}

#[test]
fn commonjs_object_exports_and_shadowed_module_are_conservative() {
    let (_dir,root,graph)=repo(&[
        ("lib.cjs","function helper(){} module.exports = {helper: helper};"),
        ("fake.cjs","const module = unknown; function wrong(){} module.exports = wrong;"),
        ("main.cjs","const {helper} = require('./lib'); const ns = require('./lib'); const fake = require('./fake'); function run(){helper();ns.helper();fake();}")]);
    assert_eq!(
        callees(&graph, &root, "main.cjs", "run"),
        expected(&["lib.cjs:helper"])
    );
}
#[test]
fn javascript_static_and_instance_method_namespaces_are_distinct() {
    let (_dir,root,graph)=repo(&[
        ("lib.js","export class Worker {static staticMethod(){} instanceMethod(){} static run(){this.staticMethod();this.instanceMethod();}}"),
        ("main.js","import {Worker} from './lib'; function classBad(){Worker.instanceMethod();} function instanceBad(){const w=new Worker();w.staticMethod();} function valid(){const w=new Worker();w.instanceMethod();Worker.staticMethod();}")]);
    assert!(callees(&graph, &root, "main.js", "classBad").is_empty());
    assert!(callees(&graph, &root, "main.js", "instanceBad").is_empty());
    assert_eq!(
        callees(&graph, &root, "main.js", "valid"),
        expected(&["lib.js:Worker.instanceMethod", "lib.js:Worker.staticMethod"])
    );
    assert_eq!(
        callees(&graph, &root, "lib.js", "Worker.run"),
        expected(&["lib.js:Worker.staticMethod"])
    );
}

#[test]
fn exported_declarations_respect_reassignment_and_type_namespace() {
    let (_dir,root,graph)=repo(&[
        ("lib.ts","export function helper(){} helper = unknown; export function known(){}"),
        ("main.ts","import {helper} from './lib'; import type * as Types from './lib'; function run(){helper();Types.known();}")]);
    assert!(callees(&graph, &root, "main.ts", "run").is_empty());
}
