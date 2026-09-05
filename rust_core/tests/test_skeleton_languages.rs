use semantic_engine::parser::{
    SkeletonResult, SupportedLanguage, SymbolHarvester, create_skeleton, create_skeleton_details,
};
use std::collections::HashSet;

fn extract(source: &str, ext: &str) -> SkeletonResult {
    let lang = SupportedLanguage::from_extension(ext);
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(lang.get_parser().unwrap()).unwrap();
    let tree = parser.parse(source, None).unwrap();
    assert!(
        !tree.root_node().has_error(),
        "Fixture parse failed ({ext}): {}",
        tree.root_node().to_sexp()
    );
    let result = create_skeleton_details(source, &tree, lang);
    assert_eq!(result.text, create_skeleton(source, &tree, lang));
    assert_eq!(
        result.text,
        result
            .chunks
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    for chunk in &result.chunks {
        for span in &chunk.spans {
            assert!(span.start_byte < span.end_byte);
            assert!(
                source.is_char_boundary(span.start_byte) && source.is_char_boundary(span.end_byte)
            );
            let retained = &source[span.start_byte..span.end_byte];
            assert!(
                chunk.text.contains(retained),
                "Unrendered span: {retained:?}"
            );
            assert_eq!(
                span.start_line,
                source[..span.start_byte]
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count()
                    + 1
            );
            assert_eq!(
                span.end_line,
                source.as_bytes()[..span.end_byte - 1]
                    .iter()
                    .filter(|b| **b == b'\n')
                    .count()
                    + 1
            );
        }
    }
    result
}

fn retains(result: &SkeletonResult, wanted: &[&str], omitted: &[&str]) {
    for value in wanted {
        assert!(
            result.text.contains(value),
            "Missing {value:?} in:\n{}",
            result.text
        );
    }
    for value in omitted {
        assert!(
            !result.text.contains(value),
            "Leaked {value:?} in:\n{}",
            result.text
        );
    }
}

#[test]
fn python_api_docs_decorators_fields_nested_and_stubs() {
    let source = r#""""Module API résumé."""
from typing import Final, overload
MAX_COUNT: Final[int] = 10

@registered("café")
class Client(Base):
    """Public client documentation."""
    field: str
    timeout: int = 3
    @property
    def name(self) -> str:
        """Returns the display name."""
        body_secret = self.field.upper()
        return body_secret
    class Options:
        retries: int = 2
    def factory(self):
        def helper(value: int) -> int:
            return value + 98765
        return helper

@overload
def parse(value: str) -> int: ...
print("runtime_secret")
"#;
    for ext in ["py", "pyi"] {
        let result = extract(source, ext);
        retains(
            &result,
            &[
                "Module API résumé.",
                "MAX_COUNT: Final[int] = 10",
                "@registered(\"café\")",
                "@property",
                "field: str",
                "timeout: int = 3",
                "Returns the display name",
                "class Options",
                "retries: int = 2",
                "def helper(value: int) -> int",
                "@overload",
                "def parse(value: str) -> int",
            ],
            &["body_secret", "98765", "runtime_secret"],
        );
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(SupportedLanguage::Python.get_parser().unwrap())
            .unwrap();
        assert!(
            !parser
                .parse(&result.text, None)
                .unwrap()
                .root_node()
                .has_error(),
            "{}",
            result.text
        );
    }
}

#[test]
fn python_conditional_api_retains_guard_without_runtime() {
    let source = "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from external import Widget\nelse:\n    class Widget:\n        value: str\n        def run(self) -> None:\n            print('runtime_secret')\n";
    let result = extract(source, "py");
    retains(
        &result,
        &[
            "if TYPE_CHECKING:",
            "from external import Widget",
            "else:",
            "class Widget",
            "value: str",
            "def run(self) -> None",
        ],
        &["runtime_secret"],
    );
}

#[test]
fn rust_struct_enum_trait_impl_macro_and_nested_module() {
    let source = r#"//! Crate documentation.
use std::fmt::Debug;
#[derive(Clone)]
pub struct Client<T> { pub value: T, private: usize }
pub enum Mode { Fast, Slow { count: usize } }
pub const LIMIT: usize = 12;
/// Trait documentation.
pub trait Runner<T>: Debug {
    type Output;
    const DEFAULT: usize = 2;
    fn signature(&self, item: T) -> Self::Output;
    fn run(&self, item: T) -> usize { let body_secret = 111; body_secret }
}
impl<T: Debug> Client<T> {
    #[inline]
    pub fn value(&self) -> &T { let body_secret = 222; &self.value }
}
pub mod nested {
    pub type Identifier = u64;
    pub fn helper() -> u64 { 123456789 }
}
macro_rules! api { ($value:expr) => { implementation_secret($value) }; }
"#;
    let result = extract(source, "rs");
    retains(
        &result,
        &[
            "Crate documentation",
            "#[derive(Clone)]",
            "pub value: T",
            "Slow { count: usize }",
            "LIMIT: usize = 12",
            "type Output",
            "const DEFAULT: usize = 2",
            "fn signature(&self, item: T) -> Self::Output;",
            "impl<T: Debug> Client<T>",
            "#[inline]",
            "pub fn value(&self) -> &T",
            "pub mod nested",
            "pub type Identifier = u64",
            "macro_rules! api",
            "$value:expr",
        ],
        &["body_secret", "123456789", "implementation_secret"],
    );
}

#[test]
fn js_exports_arrows_classes_fields_jsx_commonjs() {
    let source = r#"/** Package API. */
import Base from './base.js';
export const LIMIT = 3;
export const transform = (input) => input + 'arrow_secret';
export default class Client extends Base {
  static count = 0;
  #private = 'value';
  /** Method docs. */
  async run(value) { const body_secret = value; return body_secret; }
  view = (value) => <span>{value}jsx_secret</span>;
}
module.exports.helper = function helper(x) { return 'commonjs_secret'; };
exports.other = (x) => 'other_secret';
console.log('runtime_secret');
"#;
    for ext in ["js", "jsx", "mjs", "cjs"] {
        let result = extract(source, ext);
        retains(
            &result,
            &[
                "Package API",
                "export const LIMIT = 3",
                "export const transform = (input) =>",
                "export default class Client extends Base",
                "static count = 0",
                "#private = 'value'",
                "Method docs",
                "async run(value)",
                "view = (value) =>",
                "module.exports.helper = function helper(x)",
                "exports.other = (x) =>",
            ],
            &[
                "arrow_secret",
                "body_secret",
                "jsx_secret",
                "commonjs_secret",
                "other_secret",
                "runtime_secret",
            ],
        );
    }
}

#[test]
fn typescript_declarations_namespaces_types_and_tsx() {
    let source = r#"import type { Base } from './base';
/** Public interface. */
export interface Options { readonly name: string; run(value: number): Promise<void>; }
export type Result<T> = { value: T } | null;
export enum Mode { Fast = 1, Slow = 2 }
declare function parse(value: string): Result<string>;
export abstract class Client<T> implements Options {
  readonly name: string = 'api';
  protected field?: T;
  abstract calculate(value: T): number;
  async run(value: number): Promise<void> { const body_secret = value; }
}
export namespace Tools {
  export const build = <T,>(value: T): Result<T> => ({ value, implementation_secret: 1 });
}
export const render = (value: string) => <span>{value}jsx_secret</span>;
"#;
    let result = extract(source, "tsx");
    retains(
        &result,
        &[
            "Public interface",
            "readonly name: string",
            "run(value: number): Promise<void>;",
            "export type Result<T>",
            "Fast = 1, Slow = 2",
            "declare function parse",
            "export abstract class Client<T>",
            "protected field?: T",
            "abstract calculate(value: T): number",
            "export namespace Tools",
            "export const build = <T,>(value: T): Result<T> =>",
            "export const render = (value: string) =>",
        ],
        &["body_secret", "implementation_secret", "jsx_secret"],
    );
    let ts_source = source
        .lines()
        .filter(|line| !line.starts_with("export const render"))
        .collect::<Vec<_>>()
        .join("\n");
    let ts = extract(&ts_source, "ts");
    retains(
        &ts,
        &["declare function parse", "export namespace Tools"],
        &["body_secret", "implementation_secret"],
    );
}

#[test]
fn go_package_imports_fields_methods_interfaces_and_function_values() {
    let source = r#"// Package api provides clients.
package api
import "context"
const Limit = 3
type Client struct { Name string; private int }
type Runner interface { Run(ctx context.Context, value string) (int, error) }
type Identifier = string
// Run executes a request.
func (c *Client) Run(ctx context.Context, value string) (int, error) {
    body_secret := len(value)
    return body_secret, nil
}
func Make[T any](value T) *Client { return &Client{Name: "constructor_secret"} }
var Callback = func(value int) int { return value + 123456789 }
"#;
    let result = extract(source, "go");
    retains(
        &result,
        &[
            "Package api provides clients",
            "package api",
            "import \"context\"",
            "const Limit = 3",
            "Name string; private int",
            "type Runner interface",
            "type Identifier = string",
            "Run executes a request",
            "func (c *Client) Run(ctx context.Context, value string) (int, error)",
            "func Make[T any](value T) *Client",
            "var Callback = func(value int) int",
        ],
        &["body_secret", "constructor_secret", "123456789"],
    );
}

#[test]
fn omitted_body_bytes_never_receive_coverage_even_on_same_line() {
    let source = "pub fn café(value: &str) -> &str { let secret = \"résumé🔒\"; value }\n";
    let result = extract(source, "rs");
    let secret = source.find("let secret").unwrap();
    for chunk in &result.chunks {
        for span in &chunk.spans {
            assert!(!(span.start_byte <= secret && secret < span.end_byte));
        }
    }
    retains(
        &result,
        &["café(value: &str) -> &str", "/* ... */"],
        &["résumé🔒", "secret"],
    );
    let json = serde_json::to_string(&result).unwrap();
    assert_eq!(
        serde_json::from_str::<SkeletonResult>(&json).unwrap(),
        result
    );
}

#[test]
fn oversized_impl_splits_only_at_members_with_enclosing_identity() {
    let mut source = String::from("impl Client {\n");
    for i in 0..150 {
        source.push_str(&format!("    /// Method {i}.\n    pub fn method_{i}(&self, value: usize) -> usize {{ value + 123456789 }}\n"));
    }
    source.push_str("}\n");
    let result = extract(&source, "rs");
    assert_eq!(result.chunks.len(), 150);
    for (i, chunk) in result.chunks.iter().enumerate() {
        assert!(chunk.text.starts_with("impl Client {\n"));
        assert!(chunk.text.contains(&format!("/// Method {i}.")));
        assert!(chunk.text.contains(&format!("pub fn method_{i}(")));
        assert!(chunk.text.ends_with("}\n"));
        assert!(!chunk.text.contains("123456789"));
    }
}

#[test]
fn oversized_initializers_are_omitted_without_clipping_declaration() {
    let source = format!(
        "export const PAYLOAD: string = \"{}\";\nexport function api() {{ return 456; }}\n",
        "x".repeat(20_000)
    );
    let result = extract(&source, "ts");
    retains(
        &result,
        &[
            "export const PAYLOAD: string = ...;",
            "export function api()",
        ],
        &["xxxx", "456"],
    );
    assert!(result.text.len() < 200);
}

#[test]
fn language_specific_queries_harvest_real_definitions_and_references() {
    let cases = [
        (
            "rs",
            "pub struct Client { pub name: String } pub trait Runner { fn run(&self); } impl Client { pub fn make() { helper(); } }",
            vec!["Client", "Runner", "run", "make"],
        ),
        (
            "go",
            "package api\ntype Client struct { Name string }\ntype Runner interface { Run() }\nfunc (c *Client) Run() { helper() }\n",
            vec!["Client", "Name", "Runner", "Run"],
        ),
        (
            "ts",
            "export abstract class Client { abstract run(x: string): void; } declare function parse(x: string): number;",
            vec!["Client", "run", "parse"],
        ),
        (
            "tsx",
            "export const View = (props: Props) => <span>{helper(props)}</span>;",
            vec!["View"],
        ),
        (
            "jsx",
            "export class Client { count = 1; run() { helper(); } }",
            vec!["Client", "count", "run"],
        ),
    ];
    let harvester = SymbolHarvester::new();
    for (ext, source, expected) in cases {
        let lang = SupportedLanguage::from_extension(ext);
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(lang.get_parser().unwrap()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        assert!(!tree.root_node().has_error());
        let symbols = harvester.harvest(&tree, source, lang);
        let definitions: HashSet<&str> = symbols
            .iter()
            .filter(|s| s.is_definition)
            .map(|s| s.name.as_str())
            .collect();
        for name in expected {
            assert!(
                definitions.contains(name),
                "Missing {name} in {ext}: {definitions:?}"
            );
        }
        if source.contains("helper") {
            assert!(
                symbols
                    .iter()
                    .any(|s| !s.is_definition && s.name == "helper")
            );
        }
    }
}

#[test]
fn large_function_bodies_compress_without_hiding_signature_or_docs() {
    let mut source = String::from(
        "def process(value: int) -> int:\n    # API documentation comment.\n    \"\"\"Returns the processed value.\"\"\"\n",
    );
    for i in 0..1000 {
        source.push_str(&format!("    implementation_{i} = value + {i}\n"));
    }
    source.push_str("    return implementation_999\n");
    let result = extract(&source, "py");
    retains(
        &result,
        &[
            "def process(value: int) -> int:",
            "API documentation comment",
            "Returns the processed value",
        ],
        &["implementation_"],
    );
    assert!(result.text.len() < source.len() / 20);
}

#[test]
fn unsupported_language_fallback_and_empty_source_preserve_compatibility() {
    let source = "an unsupported format: café";
    let result = semantic_engine::skeleton::uncompressed(source);
    assert_eq!(result.text, source);
    assert_eq!(result.chunks.len(), 1);
    assert_eq!(result.chunks[0].spans[0].end_byte, source.len());
    assert_eq!(semantic_engine::skeleton::uncompressed("").chunks.len(), 0);
}
