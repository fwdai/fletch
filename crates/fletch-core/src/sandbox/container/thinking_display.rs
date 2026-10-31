//! Whether a container image's claude parses `--thinking-display`, per runtime
//! and image tag, so a launch puts the flag on the line only for an image that
//! was seen to take it.
//!
//! The image a container launches from can be older than the host CLI: the
//! runtimes refresh version parity in the background and keep launching the
//! existing image meanwhile (`docker::image`, `podman::image`), so the host
//! binary says nothing about it. Each runtime runs the probe inside its image —
//! `agent::THINKING_DISPLAY_PROBE_ARGS`, read by
//! `agent::probe_shows_thinking_display`, the same as on the host — off the
//! launch path: after a build, and by the first launch's background freshness
//! check. The launch path only reads. An image not yet probed launches without
//! the flag, exactly as before; a rebuild re-probes and overwrites. Keyed by tag
//! rather than image id because the launch path has no inspect to spend.

use std::collections::HashMap;
use std::sync::OnceLock;

use parking_lot::Mutex;

fn known_map() -> &'static Mutex<HashMap<String, bool>> {
    static KNOWN: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    KNOWN.get_or_init(|| Mutex::new(HashMap::new()))
}

fn key(runtime: &str, tag: &str) -> String {
    format!("{runtime}:{tag}")
}

/// Whether `tag` on `runtime` has been seen to take the flag. Unprobed reads
/// as not.
pub(crate) fn confirmed(runtime: &str, tag: &str) -> bool {
    known_map().lock().get(&key(runtime, tag)).copied().unwrap_or(false)
}

/// Whether `tag` on `runtime` has been probed at all, so a freshness check
/// probes an image once per app run and a rebuild's re-probe isn't repeated.
pub(crate) fn known(runtime: &str, tag: &str) -> bool {
    known_map().lock().contains_key(&key(runtime, tag))
}

pub(crate) fn record(runtime: &str, tag: &str, supported: bool) {
    known_map().lock().insert(key(runtime, tag), supported);
    tracing::info!(
        target: "fletch::container",
        runtime,
        tag,
        supported,
        "container CLI --thinking-display capability probed",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unprobed_image_is_not_vouched_for_and_a_rebuild_overwrites() {
        assert!(!known("test-rt", "img:a"));
        assert!(!confirmed("test-rt", "img:a"));
        record("test-rt", "img:a", true);
        assert!(known("test-rt", "img:a"));
        assert!(confirmed("test-rt", "img:a"));
        // The other runtime's store says nothing about this one.
        assert!(!known("other-rt", "img:a"));
        record("test-rt", "img:a", false);
        assert!(!confirmed("test-rt", "img:a"));
    }
}
