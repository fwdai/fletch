//! The derivation rules over hand-written file lists.

use super::rules::modules;
use super::Module;

fn files(list: &[&str]) -> Vec<String> {
    list.iter().map(|f| f.to_string()).collect()
}

fn find<'a>(modules: &'a [Module], slug: &str) -> &'a Module {
    modules
        .iter()
        .find(|m| m.slug == slug)
        .unwrap_or_else(|| panic!("no module `{slug}` in {modules:#?}"))
}

/// A tree shaped like this repository's: a root npm app, standalone crates,
/// a nested Tauri shell and plugin, a second package.
fn fletch_like() -> Vec<String> {
    files(&[
        "package.json",
        "README.md",
        "src/main.tsx",
        "src/components/App.tsx",
        "src/components/ProjectContext/index.tsx",
        "src/styles/app.css",
        "src-tauri/Cargo.toml",
        "src-tauri/src/lib.rs",
        "src-tauri/src/commands/context.rs",
        "crates/fletch-core/Cargo.toml",
        "crates/fletch-core/src/lib.rs",
        "crates/fletch-core/src/context/mod.rs",
        "crates/fletch-core/src/context/tests/store.rs",
        "crates/fletch-core/src/capture/ingest/mod.rs",
        "crates/fletch-core/src/instructions/context.md",
        "crates/fletch-proto/Cargo.toml",
        "crates/fletch-proto/src/lib.rs",
        "mobile/package.json",
        "mobile/src/screens/Home.tsx",
        "mobile/src-tauri/Cargo.toml",
        "mobile/src-tauri/src/lib.rs",
        "mobile/src-tauri/plugins/push/Cargo.toml",
        "mobile/src-tauri/plugins/push/src/lib.rs",
        "relay/package.json",
        "relay/src/index.ts",
        "docs/project-context-layer.md",
    ])
}

#[test]
fn every_package_and_app_directory_is_a_module() {
    let got = modules(&fletch_like());
    let slugs: Vec<&str> = got.iter().map(|m| m.slug.as_str()).collect();
    for slug in [
        "src",
        "src-tauri",
        "fletch-core",
        "fletch-proto",
        "mobile",
        "mobile-src-tauri",
        "mobile-src-tauri-push",
        "relay",
    ] {
        assert!(slugs.contains(&slug), "missing `{slug}` in {slugs:?}");
    }
    assert_eq!(find(&got, "fletch-core").path, "crates/fletch-core");
    assert_eq!(find(&got, "fletch-core").name, "fletch-core");
    assert_eq!(find(&got, "src").path, "src");
}

#[test]
fn nesting_gives_the_parent() {
    let got = modules(&fletch_like());
    assert_eq!(find(&got, "fletch-core").parent, None);
    assert_eq!(find(&got, "src").parent, None);
    assert_eq!(find(&got, "mobile").parent, None);
    assert_eq!(
        find(&got, "mobile-src-tauri").parent.as_deref(),
        Some("mobile")
    );
    assert_eq!(
        find(&got, "mobile-src-tauri-push").parent.as_deref(),
        Some("mobile-src-tauri")
    );
}

#[test]
fn code_directories_under_src_are_modules_of_their_package() {
    let got = modules(&fletch_like());
    let context = find(&got, "fletch-core-context");
    assert_eq!(context.path, "crates/fletch-core/src/context");
    assert_eq!(context.name, "context");
    assert_eq!(context.parent.as_deref(), Some("fletch-core"));
    assert_eq!(
        find(&got, "fletch-core-capture").parent.as_deref(),
        Some("fletch-core")
    );
    assert_eq!(find(&got, "src-components").parent.as_deref(), Some("src"));
    assert_eq!(
        find(&got, "src-tauri-commands").parent.as_deref(),
        Some("src-tauri")
    );
    assert_eq!(
        find(&got, "mobile-screens").parent.as_deref(),
        Some("mobile")
    );
    // Only direct children of `src/`, and only ones that hold code.
    let slugs: Vec<&str> = got.iter().map(|m| m.slug.as_str()).collect();
    assert!(!slugs.contains(&"fletch-core-instructions"), "{slugs:?}");
    assert!(!slugs.contains(&"src-styles"), "{slugs:?}");
    assert!(!slugs
        .iter()
        .any(|s| s.contains("ingest") || s.contains("tests")));
    assert!(!slugs.iter().any(|s| s.contains("docs")));
}

#[test]
fn parents_come_before_children() {
    let got = modules(&fletch_like());
    for (i, m) in got.iter().enumerate() {
        if let Some(parent) = &m.parent {
            let at = got.iter().position(|p| &p.slug == parent).unwrap();
            assert!(at < i, "`{}` precedes its parent `{parent}`", m.slug);
        }
    }
}

#[test]
fn dependencies_build_output_and_dot_directories_are_not_read() {
    let got = modules(&files(&[
        "Cargo.toml",
        "src/main.rs",
        "node_modules/left-pad/package.json",
        "node_modules/left-pad/src/index.js",
        "target/debug/build/foo/Cargo.toml",
        ".github/actions/thing/package.json",
        "tests/fixtures/app/package.json",
    ]));
    let slugs: Vec<&str> = got.iter().map(|m| m.slug.as_str()).collect();
    assert_eq!(slugs, vec!["src"]);
}

#[test]
fn a_cargo_workspace_without_a_root_crate_has_no_src_module() {
    let got = modules(&files(&[
        "Cargo.toml",
        "crates/api/Cargo.toml",
        "crates/api/src/lib.rs",
        "crates/api/src/routes/mod.rs",
    ]));
    let slugs: Vec<&str> = got.iter().map(|m| m.slug.as_str()).collect();
    assert_eq!(slugs, vec!["api", "api-routes"]);
}

#[test]
fn a_slug_two_directories_share_keeps_the_first() {
    let got = modules(&files(&[
        "apps/web/package.json",
        "apps/web/src/main.ts",
        "packages/web/package.json",
        "packages/web/src/index.ts",
    ]));
    let web: Vec<&Module> = got.iter().filter(|m| m.slug == "web").collect();
    assert_eq!(web.len(), 1);
    assert_eq!(web[0].path, "apps/web");
}

#[test]
fn directory_names_become_valid_slugs() {
    let got = modules(&files(&[
        "Packages/My Lib/package.json",
        "Packages/My Lib/src/Sub Dir/a.ts",
    ]));
    let slugs: Vec<&str> = got.iter().map(|m| m.slug.as_str()).collect();
    assert_eq!(slugs, vec!["my-lib", "my-lib-sub-dir"]);
    assert_eq!(got[0].name, "My Lib");
}
