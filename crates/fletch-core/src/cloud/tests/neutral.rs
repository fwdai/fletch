//! No provider is named outside `cloud/`: the rest of the app sees a provider
//! as an opaque [`ProviderId`] with a label, and a box through the neutral
//! types, so adding a provider is a module here and its enum variants, and
//! nothing elsewhere.

use std::path::Path;

use super::ProviderId;
use crate::source_scan::{production_sources, production_sources_in};

/// Every production source the rule covers, as `(<crate>/<path in src>,
/// text)`: this crate's outside `cloud/`, and the desktop app, the headless
/// host and the box runtime beside it.
fn covered() -> Vec<(String, String)> {
    let core = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources: Vec<(String, String)> = production_sources()
        .into_iter()
        .filter(|(rel, _)| !rel.starts_with("cloud/"))
        .map(|(rel, text)| (format!("fletch-core/{rel}"), text))
        .collect();
    for (name, src) in [
        ("src-tauri", "../../src-tauri/src"),
        ("fletch-host", "../fletch-host/src"),
        ("fletch-box", "../fletch-box/src"),
    ] {
        sources.extend(
            production_sources_in(&core.join(src))
                .into_iter()
                .map(|(rel, text)| (format!("{name}/{rel}"), text)),
        );
    }
    sources
}

/// Whether `text` holds `word` as a whole word, in any case: between two
/// characters that are not letters, digits or `_`.
fn has_word(text: &str, word: &str) -> bool {
    let (text, word) = (text.to_ascii_lowercase(), word.to_ascii_lowercase());
    // `map_or`, not `is_none_or`: the crate's rust-version is 1.77.
    let boundary = |c: Option<char>| c.map_or(true, |c| !c.is_ascii_alphanumeric() && c != '_');
    text.match_indices(&word).any(|(at, _)| {
        boundary(text[..at].chars().next_back()) && boundary(text[at + word.len()..].chars().next())
    })
}

/// What names `provider` in source: its id as a whole word in any case (a
/// module path, a stored string, the enum variant, a word of prose), or its
/// label.
fn names(text: &str, provider: ProviderId) -> bool {
    has_word(text, provider.as_str()) || text.contains(provider.label())
}

#[test]
fn no_provider_is_named_outside_cloud() {
    let sources = covered();
    assert!(
        sources.iter().any(|(rel, _)| rel.starts_with("src-tauri/")),
        "the desktop app's sources are where this test expects them"
    );
    let named: Vec<String> = sources
        .iter()
        .flat_map(|(rel, text)| {
            ProviderId::ALL
                .iter()
                .filter(|&&p| names(text, p))
                .map(move |p| format!("{rel} names {}", p.label()))
        })
        .collect();
    assert!(
        named.is_empty(),
        "a provider is named outside cloud/ (crates/fletch-core/src/cloud/mod.rs): what differs \
         per provider belongs in its adapter, behind the neutral surface.\n{}",
        named.join("\n")
    );
}

#[test]
fn a_name_is_a_whole_word() {
    assert!(has_word("use crate::cloud::fly::Client;", "fly"));
    assert!(has_word("ProviderId::Fly =>", "fly"));
    assert!(has_word("\"fly\"", "fly"));
    assert!(!has_word("retried briefly, on the flying", "fly"));
    assert!(!has_word("fly_by_night", "fly"));
}
