//! Multi-language run-config detection.
//!
//! Each ecosystem has a [`RunDetector`] that reads a few files at a
//! checkout root and, if it recognizes the project, returns a
//! [`DetectedConfig`] with a confidence score. [`detect_all`] runs every
//! detector and returns the surviving configs ranked by confidence.
//!
//! Detectors are pure (`&Tree -> Option<DetectedConfig>`) so they can be
//! unit-tested against fixture directories with no Tauri/DB setup. A [`Tree`]
//! is the checkout root as its executor sees it (`exec::for_path`): a box
//! keeps its checkout in its guest, where the host path is an empty
//! placeholder (docs/sandbox-foundation.md rule 2).
//!
//! The rows map onto the panel's hybrid schema: the core rows
//! (`version`, `install`, `dev`, `test`, `build`) are attempted for every
//! ecosystem; the optional rows (`lint`, `port`, `env`) are emitted only when
//! detected — `lint` when a conventional linter hook exists (a `lint` npm
//! script, or a declared linter dependency). Rows a detector can't fill are
//! simply omitted.

use serde::Serialize;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use crate::exec::Executor;

pub(crate) mod command;
mod go;
mod node;
mod php;
pub(crate) mod port;
mod python;
mod ruby;
mod rust;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RowGroup {
    Environment,
    Scripts,
    Server,
}

/// A single configuration row the panel renders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetectedRow {
    /// Stable id matching the panel's override-key scheme
    /// (`version` | `install` | `dev` | `test` | `build` | `lint` | `port` |
    /// `env`).
    pub id: String,
    pub group: RowGroup,
    /// Human label, e.g. "Node version", "Toolchain".
    pub key: String,
    /// Detected value, e.g. "v22.4.0", "cargo run".
    pub value: String,
    /// Where the value came from, e.g. ".nvmrc", "package.json · scripts.dev".
    pub source: String,
}

impl DetectedRow {
    fn new(id: &str, group: RowGroup, key: &str, value: impl Into<String>, source: &str) -> Self {
        Self {
            id: id.to_string(),
            group,
            key: key.to_string(),
            value: value.into(),
            source: source.to_string(),
        }
    }
}

/// One ecosystem's worth of detected configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetectedConfig {
    /// "node" | "python" | "ruby" | "rust" | "go" | "php".
    pub ecosystem: String,
    /// 0–100. Lockfile present → high; manifest only → medium;
    /// bare source files → low.
    pub confidence: u8,
    pub rows: Vec<DetectedRow>,
}

/// Confidence tiers. A lockfile is strong evidence the project is both
/// real and uses a known toolchain; a bare manifest is weaker; loose
/// source files weakest.
const CONFIDENCE_LOCKFILE: u8 = 90;
const CONFIDENCE_MANIFEST: u8 = 60;

pub trait RunDetector {
    fn detect(&self, tree: &Tree) -> Option<DetectedConfig>;
}

/// A checkout root as the detectors read it: its entries listed once where
/// the tree lives, each file read on demand through the same executor. One
/// listing answers every "is there a lockfile" question, so a tree in a box
/// costs one round trip plus one per file actually read.
pub struct Tree {
    exec: Executor,
    root: PathBuf,
    names: HashSet<String>,
}

impl Tree {
    /// Open `root` where it lives. A host root the app cannot list detects
    /// nothing, as a host read always did; a tree in a box that cannot be
    /// reached — stopped, under a reader that must not boot it
    /// (`exec::passive`), or failing to boot — is the error, so the caller
    /// can say why nothing was detected rather than run nothing.
    pub fn open(root: &Path) -> io::Result<Self> {
        let exec = crate::exec::for_path(root);
        let names = match exec.list(root) {
            Ok(entries) => entries.into_iter().map(|e| e.name).collect(),
            Err(_) if matches!(exec, Executor::Local) => HashSet::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            exec,
            root: root.to_path_buf(),
            names,
        })
    }

    /// Whether `name`, an entry of the root, is there.
    fn exists(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// The file `name` at the root, trimmed. `None` if absent, unreadable,
    /// not UTF-8 or blank.
    fn read_trimmed(&self, name: &str) -> Option<String> {
        if !self.exists(name) {
            return None;
        }
        let bytes = self.exec.read(&self.root.join(name)).ok()?;
        let text = String::from_utf8(bytes).ok()?;
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }
}

fn detectors() -> Vec<Box<dyn RunDetector>> {
    vec![
        Box::new(node::NodeDetector),
        Box::new(python::PythonDetector),
        Box::new(ruby::RubyDetector),
        Box::new(rust::RustDetector),
        Box::new(go::GoDetector),
        Box::new(php::PhpDetector),
    ]
}

/// Run every detector over `checkout` and return the recognized configs
/// ranked by confidence (highest first). Empty when nothing matched —
/// callers treat that as the no-op fallback — and when the tree cannot be
/// read at all; a caller that must tell those apart opens the [`Tree`]
/// itself and uses [`detect_all_in`].
pub fn detect_all(checkout: &Path) -> Vec<DetectedConfig> {
    Tree::open(checkout)
        .map(|tree| detect_all_in(&tree))
        .unwrap_or_default()
}

/// [`detect_all`] over an opened tree.
pub fn detect_all_in(tree: &Tree) -> Vec<DetectedConfig> {
    let mut configs: Vec<DetectedConfig> =
        detectors().iter().filter_map(|d| d.detect(tree)).collect();
    // Stable sort by confidence desc so ties keep detector registration
    // order (node before python before … — a deterministic primary).
    configs.sort_by_key(|c| std::cmp::Reverse(c.confidence));
    configs
}

/// Shared fixture/assertion helpers for the per-detector test modules.
#[cfg(test)]
pub(crate) mod test_support {
    use super::{DetectedConfig, DetectedRow, Tree};
    use std::fs;
    use tempfile::TempDir;

    /// Build a fixture dir from (relative path, contents) pairs.
    pub(crate) fn fixture(files: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for (path, contents) in files {
            let full = dir.path().join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(full, contents).unwrap();
        }
        dir
    }

    /// Run `detector` over a fixture built from `files`.
    pub(crate) fn detect_in(
        detector: &dyn super::RunDetector,
        files: &[(&str, &str)],
    ) -> Option<DetectedConfig> {
        let dir = fixture(files);
        detector.detect(&Tree::open(dir.path()).unwrap())
    }

    pub(crate) fn row<'a>(cfg: &'a DetectedConfig, id: &str) -> Option<&'a DetectedRow> {
        cfg.rows.iter().find(|r| r.id == id)
    }

    pub(crate) fn val(cfg: &DetectedConfig, id: &str) -> String {
        row(cfg, id).map(|r| r.value.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::fixture;
    use super::*;

    // ── empty / fallback ──────────────────────────────────────────────────

    #[test]
    fn empty_dir_detects_nothing() {
        let dir = fixture(&[]);
        assert!(detect_all(dir.path()).is_empty());
    }

    // ── ranking / polyglot ────────────────────────────────────────────────

    #[test]
    fn node_root_outranks_rust_subdir() {
        // This repo's own shape: Node at root, Rust under src-tauri/.
        // Detection runs at the checkout root, so only the root Node
        // project is seen — but even a root Cargo.toml must lose to the
        // root lockfile-backed Node project.
        let dir = fixture(&[
            ("package.json", r#"{"scripts":{"dev":"vite"}}"#),
            ("pnpm-lock.yaml", ""),
            ("Cargo.toml", "[package]\nname = \"x\"\n"),
        ]);
        let configs = detect_all(dir.path());
        assert_eq!(configs[0].ecosystem, "node");
        assert!(configs[0].confidence > configs[1].confidence);
    }
}
