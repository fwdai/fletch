//! Image-build progress broadcast to the UI.
//!
//! The embedded agent image is built on the first docker spawn (see
//! [`super::image::ensure_image`]) — a potentially minutes-long `docker build`
//! that blocks the spawn until it finishes. That work happens deep in the
//! engine launch path, which has no `AppHandle`, so this module offers a
//! process-wide sink the app installs once at startup ([`set_build_sink`]) to
//! forward build events to the UI. Until a sink is installed (or in headless
//! tests) emitting is a no-op, so the build path stays decoupled from Tauri —
//! matching how `engine::set_launch_settings` mirrors settings without a DB
//! handle.

use parking_lot::RwLock;

/// One image-build lifecycle event. Serializes tagged (`{ "phase": "line",
/// "line": "…" }`) so the frontend can pattern-match a single event stream.
#[derive(Clone, serde::Serialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
pub enum BuildEvent {
    /// A build just started (image missing, `docker build` about to run).
    Started,
    /// One line of `docker build` output.
    Line { line: String },
    /// The build finished successfully.
    Finished,
    /// The build failed; `error` is the user-readable reason.
    Failed { error: String },
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
        emit(BuildEvent::Started);

        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        set_build_sink(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
        });

        emit(BuildEvent::Started);
        emit(BuildEvent::Line {
            line: "step 1/5".into(),
        });
        emit(BuildEvent::Finished);
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn build_event_serializes_tagged() {
        let json = serde_json::to_value(BuildEvent::Line {
            line: "pulling base image".into(),
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "phase": "line", "line": "pulling base image" })
        );
        assert_eq!(
            serde_json::to_value(BuildEvent::Started).unwrap(),
            serde_json::json!({ "phase": "started" })
        );
        assert_eq!(
            serde_json::to_value(BuildEvent::Failed {
                error: "boom".into()
            })
            .unwrap(),
            serde_json::json!({ "phase": "failed", "error": "boom" })
        );
    }
}
