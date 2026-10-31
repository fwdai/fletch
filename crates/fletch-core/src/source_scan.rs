//! Test support for the guard tests that read the crate's own source and pin
//! where a pattern may appear (`exec::tests::no_direct_git_on_checkouts`,
//! `sandbox::tests::engine_knowledge_stays_in_sandbox`). Compiled for tests
//! only.

use std::path::{Path, PathBuf};

/// Every production source file of this crate: `(path relative to src/ with
/// `/` separators, the text up to the file's test module)`, in path order.
/// Test files and the test half of every other file are left out.
pub(crate) fn production_sources() -> Vec<(String, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    files.sort();
    files
        .into_iter()
        .filter_map(|file| {
            let rel = file
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if is_test_file(&rel) {
                return None;
            }
            let text = std::fs::read_to_string(&file).unwrap();
            let production = production_half(&text).to_string();
            Some((rel, production))
        })
        .collect()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn is_test_file(rel: &str) -> bool {
    rel.contains("/tests/")
        || rel.ends_with("/tests.rs")
        || rel.ends_with("_tests.rs")
        || rel.ends_with("testing.rs")
}

/// The source up to the file's test module, where one exists.
fn production_half(text: &str) -> &str {
    [
        "#[cfg(test)]\nmod ",
        "#[cfg(test)]\npub(crate) mod ",
        "#[cfg(test)]\npub mod ",
    ]
    .iter()
    .filter_map(|marker| text.find(marker))
    .min()
    .map_or(text, |at| &text[..at])
}
