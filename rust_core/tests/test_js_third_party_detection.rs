/// Tests for auto-detection of third-party packages from package.json (JS/TS).

use semantic_engine::import_resolver::{ImportResolver, JsTsImportResolver};
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn create_file(root: &Path, path: &str, content: &str) {
    let file_path = root.join(path);
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(file_path, content).unwrap();
}

#[test]
fn test_package_json_dependencies_detected() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    create_file(root, "package.json", r#"{
        "dependencies": {
            "react": "^18.2.0",
            "next": "^14.0.0",
            "axios": "^1.6.0"
        },
        "devDependencies": {
            "typescript": "^5.0.0",
            "jest": "^29.0.0"
        }
    }"#);
    create_file(root, "src/app.ts", "");

    let resolver = JsTsImportResolver::new(root, &[]);

    // Should have Node built-ins + package.json deps
    let count = resolver.get_third_party_count();
    // NODE_BUILTIN_MODULES has ~32 entries, plus 5 from package.json
    assert!(count > 32, "Expected more than just built-ins, got {}", count);
}

#[test]
fn test_scoped_packages_detected() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    create_file(root, "package.json", r#"{
        "dependencies": {
            "@babel/core": "^7.23.0",
            "@types/react": "^18.2.0",
            "lodash": "^4.17.21"
        }
    }"#);
    create_file(root, "src/app.ts", "");

    let resolver = JsTsImportResolver::new(root, &[]);
    let count = resolver.get_third_party_count();
    // Should include @babel/core, @babel (scope), @types/react, @types (scope), lodash
    assert!(count > 32, "Expected scoped packages to be added, got {}", count);
}

#[test]
fn test_monorepo_packages_detected() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    create_file(root, "package.json", r#"{
        "dependencies": {
            "react": "^18.0.0"
        }
    }"#);
    create_file(root, "packages/ui/package.json", r#"{
        "dependencies": {
            "styled-components": "^6.0.0"
        }
    }"#);
    create_file(root, "apps/web/package.json", r#"{
        "dependencies": {
            "next": "^14.0.0"
        }
    }"#);
    create_file(root, "src/app.ts", "");

    let resolver = JsTsImportResolver::new(root, &[]);
    let count = resolver.get_third_party_count();
    // react + styled-components + next + Node built-ins
    assert!(count > 34, "Expected monorepo packages to be found, got {}", count);
}

#[test]
fn test_fallback_to_builtins_when_no_package_json() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    create_file(root, "src/app.js", "");

    let resolver = JsTsImportResolver::new(root, &[]);
    // Should still have Node built-ins
    let count = resolver.get_third_party_count();
    assert!(count >= 30, "Expected at least Node built-ins, got {}", count);
}

#[test]
fn test_subdirectory_package_json() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    create_file(root, "frontend/package.json", r#"{
        "dependencies": {
            "vue": "^3.3.0",
            "pinia": "^2.1.0"
        }
    }"#);
    create_file(root, "src/app.ts", "");

    let resolver = JsTsImportResolver::new(root, &[]);
    let count = resolver.get_third_party_count();
    // vue + pinia + Node built-ins
    assert!(count > 32, "Expected subdirectory packages to be found, got {}", count);
}

#[test]
fn test_peer_dependencies_detected() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    create_file(root, "package.json", r#"{
        "peerDependencies": {
            "react": "^18.0.0",
            "react-dom": "^18.0.0"
        }
    }"#);
    create_file(root, "src/app.tsx", "");

    let resolver = JsTsImportResolver::new(root, &[]);
    let count = resolver.get_third_party_count();
    assert!(count > 32, "Expected peer dependencies to be included, got {}", count);
}
