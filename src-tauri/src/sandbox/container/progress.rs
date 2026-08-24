//! Image-build progress broadcast to the UI, shared by both container runtimes.
//!
//! The build runs deep in the launch path with no `AppHandle`, so the app
//! installs a process-wide sink at startup ([`set_build_sink`]); emitting is a
//! no-op until it does, keeping the build path free of Tauri.
//!
//! One sink, but not one build at a time — each runtime serializes on its own
//! lock, so two first-builds can overlap. Every event therefore carries
//! `runtime`: it is the lifecycle key the frontend routes on.

use parking_lot::RwLock;

/// One image-build lifecycle event, serialized tagged by `phase` so the
/// frontend can pattern-match a single stream. `runtime` rides every variant —
/// it separates two overlapping lifecycles.
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

/// Forward a build event to the installed sink; a no-op when there is none.
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

    // The sink is a process-wide global: any further test touching it must
    // serialize with this one.
    #[test]
    fn sink_receives_events_and_no_op_without_one() {
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
