//! The derivation rules: which directories of a repository are modules, what
//! each is called and what it is part of. Pure over a list of repo-relative
//! file paths, so the same rules read a whole tree at a commit (bootstrap) or
//! the files a merged PR touched.
//!
//! - A **package** is a directory holding a manifest ([`MANIFESTS`]): every
//!   Cargo crate and npm package, workspace member or not. It is a module
//!   named after its directory; one nested inside another package is part of
//!   it and its slug carries the parent's (`mobile/src-tauri` →
//!   `mobile-src-tauri`).
//! - The **root** package is the project itself, not a module: its `src/`
//!   stands in for it (slug `src`), when it holds code. Packages below the
//!   root are not part of it.
//! - Every direct subdirectory of a package's `src/` that holds code is a
//!   module part of that package (`crates/fletch-core/src/context` →
//!   `fletch-core-context`).
//!
//! Paths under [`SKIPPED`] directories (or any dot-directory) are never read.
//! Two directories that would share a slug keep the first in path order.

use std::collections::{BTreeSet, HashMap, HashSet};

use super::Module;

/// File names that make their directory a package.
const MANIFESTS: [&str; 4] = ["Cargo.toml", "package.json", "pyproject.toml", "go.mod"];

/// Directory names whose contents are never modules: dependencies, build
/// output, test data.
const SKIPPED: [&str; 7] = [
    "node_modules",
    "vendor",
    "target",
    "dist",
    "build",
    "fixtures",
    "testdata",
];

/// Extensions that count as code when deciding whether a directory holds any.
const CODE: [&str; 26] = [
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "swift", "kt", "java", "rb", "c",
    "h", "cc", "cpp", "cs", "vue", "svelte", "scala", "ex", "exs", "php", "dart", "zig",
];

/// The modules `files` describe, parents before children.
pub fn modules(files: &[String]) -> Vec<Module> {
    let files: Vec<&str> = files
        .iter()
        .map(|f| f.trim_start_matches("./"))
        .filter(|f| !skipped(f))
        .collect();
    let packages: BTreeSet<&str> = files
        .iter()
        .filter_map(|f| {
            let (dir, name) = split(f);
            MANIFESTS.contains(&name).then_some(dir)
        })
        .collect();
    let code = code_dirs(&files);

    let mut out = Modules::default();
    // Path order puts an enclosing package before what it holds.
    for &dir in &packages {
        if dir.is_empty() {
            if code.contains("src") && !packages.contains("src") {
                out.add("", "src", "src".into(), None);
            }
            continue;
        }
        let parent = enclosing(&packages, dir)
            .filter(|p| !p.is_empty())
            .and_then(|p| out.slug_of(p));
        let slug = match &parent {
            Some(parent) => format!("{parent}-{}", last(dir)),
            None => last(dir).to_string(),
        };
        out.add(dir, dir, slug, parent);
    }

    for &dir in &packages {
        let Some(parent) = out.slug_of(dir) else {
            continue;
        };
        let src = join(dir, "src");
        let children: BTreeSet<&str> = code
            .iter()
            .filter_map(|d| d.strip_prefix(src.as_str())?.strip_prefix('/'))
            .filter_map(|rest| rest.split('/').next())
            .collect();
        for child in children {
            let path = format!("{src}/{child}");
            if !packages.contains(path.as_str()) {
                out.add(
                    &path,
                    &path,
                    format!("{parent}-{child}"),
                    Some(parent.clone()),
                );
            }
        }
    }
    out.list
}

/// The modules so far, with the slug each package (by its directory) got.
#[derive(Default)]
struct Modules {
    list: Vec<Module>,
    by_dir: HashMap<String, String>,
}

impl Modules {
    /// Record a module for `path`, filed under `key` (the package directory
    /// its children look it up by). A slug already taken, or one that cleans
    /// to nothing, is dropped.
    fn add(&mut self, key: &str, path: &str, raw_slug: String, parent: Option<String>) {
        let slug = slugify(&raw_slug);
        if slug.is_empty() || self.list.iter().any(|m| m.slug == slug) {
            return;
        }
        self.by_dir.insert(key.to_string(), slug.clone());
        self.list.push(Module {
            slug,
            name: last(path).to_string(),
            path: path.to_string(),
            parent,
        });
    }

    fn slug_of(&self, dir: &str) -> Option<String> {
        self.by_dir.get(dir).cloned()
    }
}

/// Every directory that holds a code file, directly or below.
fn code_dirs<'a>(files: &[&'a str]) -> HashSet<&'a str> {
    let mut out = HashSet::new();
    for f in files.iter().filter(|f| is_code(f)) {
        let mut dir = split(f).0;
        while !dir.is_empty() && out.insert(dir) {
            dir = split(dir).0;
        }
    }
    out
}

/// The nearest package strictly above `dir`; the root package is `""`.
fn enclosing<'a>(packages: &BTreeSet<&'a str>, dir: &str) -> Option<&'a str> {
    let mut up = split(dir).0;
    loop {
        if let Some(found) = packages.get(up) {
            return Some(found);
        }
        if up.is_empty() {
            return None;
        }
        up = split(up).0;
    }
}

fn skipped(path: &str) -> bool {
    let dir = split(path).0;
    !dir.is_empty()
        && dir
            .split('/')
            .any(|s| s.starts_with('.') || SKIPPED.contains(&s))
}

fn is_code(path: &str) -> bool {
    last(path)
        .rsplit_once('.')
        .is_some_and(|(_, ext)| CODE.contains(&ext))
}

/// `(parent directory, last segment)`; the parent of a top-level name is `""`.
fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

fn last(path: &str) -> &str {
    split(path).1
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// A directory name as a slug the store accepts: lowercase `[a-z0-9._-]`,
/// starting alphanumeric, at most 64 characters.
fn slugify(raw: &str) -> String {
    let slug: String = raw
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    slug.trim_start_matches(|c: char| !c.is_ascii_alphanumeric())
        .chars()
        .take(64)
        .collect()
}
