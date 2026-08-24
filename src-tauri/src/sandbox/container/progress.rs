//! Image-build progress broadcast to the UI, shared by both container runtimes.
//!
//! The embedded agent image is built on the first spawn under a given runtime —
//! a potentially minutes-long `build` that blocks the spawn until it finishes.
//! That work happens deep in the engine launch path, which has no `AppHandle`,
//! so this module offers a process-wide sink the app installs once at startup
//! ([`set_build_sink`]) to forward build events to the UI. Until a sink is
//! installed (or in headless tests) emitting is a no-op, so the build path stays
//! decoupled from Tauri — matching how the engines' `set_launch_settings`
//! mirrors settings without a DB handle.
//!
//! One sink for both runtimes — but NOT one build at a time: each runtime
//! serializes builds on its own lock, so a Docker and a Podman first-build can
//! overlap. Every event therefore carries `runtime`, its lifecycle key: the
//! frontend keys build state on it, so interleaved lifecycles can't clear or
//! overwrite each other's toast.

use parking_lot::RwLock;

/// One image-build lifecycle event. Serializes tagged (`{ "phase": "line",
/// "runtime": "Docker", "line": "…" }`) so the frontend can pattern-match a
/// single event stream. `runtime` is the runtime's display name ("Docker" /
/// "Podman") on every variant — it is the key that separates two overlapping
/// lifecycles, not decoration; the frontend still treats it as optional so an
/// event without it renders under a neutral key.
#[derive(Clone, serde::Serialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
pub enum BuildEvent {
    /// A build just started (image missing, `build` about to run).
    Started { runtime: &'static str },
    /// One line of `build` output.
    Line { runtime: &'static str, line: String },
    /// The build finished successfully.
    Finished { runtime: &'static str },
    /// The build failed; `error` is the user-readable reason.
    Failed {
        runtime: &'static str,
        error: String,
    },
}

type Sink = Box<dyn Fn(BuildEvent) + Send + Sync>;

static SINK: RwLock<Option<Sink>> = RwLock::new(None);

/// Install the process-wide progress sink (the app wires this to a Tauri event
/// emitter at startup). Replaces any previous sink.
pub fn set_build_sink(sink: impl Fn(BuildEvent) + Send + Sync + 'static) {
    *SINK.write() = Some(Box::new(sink));
}

/// Forward a build event to the installed sink, if any. A no-op when none is
/// installed, so the build machinery never depends on the app being wired up.
pub(crate) fn emit(event: BuildEvent) {
    if let Some(sink) = SINK.read().as_ref() {
        sink(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // Serializes with any other test touching the process-wide sink. This is the
    // only such test today; the guard documents the shared-global contract.
    #[test]
    fn sink_receives_events_and_no_op_without_one() {
        // No sink installed at first: emitting must not panic (the build path
        // runs in headless tests with no app wired up).
        emit(BuildEvent::Started { runtime: "Docker" });

        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        set_build_sink(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
        });

        emit(BuildEvent::Started { runtime: "Podman" });
        emit(BuildEvent::Line {
            runtime: "Podman",
            line: "step 1/5".into(),
        });
        emit(BuildEvent::Finished { runtime: "Podman" });
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    /// `runtime` rides every phase, not just `started`: it is the lifecycle
    /// key that keeps two overlapping runtimes' events from clearing or
    /// overwriting each other's toast state. Fields are additive, so a
    /// consumer that only reads `phase` still works.
    #[test]
    fn build_event_serializes_tagged_with_runtime_on_every_phase() {
        assert_eq!(
            serde_json::to_value(BuildEvent::Started { runtime: "Podman" }).unwrap(),
            serde_json::json!({ "phase": "started", "runtime": "Podman" })
        );
        assert_eq!(
            serde_json::to_value(BuildEvent::Line {
                runtime: "Docker",
                line: "pulling base image".into(),
            })
            .unwrap(),
            serde_json::json!({ "phase": "line", "runtime": "Docker", "line": "pulling base image" })
        );
        assert_eq!(
            serde_json::to_value(BuildEvent::Finished { runtime: "Docker" }).unwrap(),
            serde_json::json!({ "phase": "finished", "runtime": "Docker" })
        );
        assert_eq!(
            serde_json::to_value(BuildEvent::Failed {
                runtime: "Podman",
                error: "boom".into()
            })
            .unwrap(),
            serde_json::json!({ "phase": "failed", "runtime": "Podman", "error": "boom" })
        );
    }
}
