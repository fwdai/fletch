//! Voice dictation for the composer: hold the mic button, speak, get text.
//!
//! The whole thing is one session at a time, app-wide. `dictation_start`
//! begins listening, `dictation:transcript` events carry the running text as
//! the recognizer revises it, and `dictation_stop` ends audio so the
//! recognizer can flush a final result. `dictation:state` brackets that with
//! `listening` / `stopped` / `error`, so the frontend never has to infer
//! whether a session is alive.
//!
//! Transcripts are whole-session text, not deltas: Apple rewrites earlier
//! words as later context arrives ("to" → "two" → "too"), so a delta stream
//! would be unreconstructable. Each event replaces the last.
//!
//! Only Apple platforms have an implementation (`apple`); everywhere else the
//! commands are stubs that report `supported: false`, which is what keeps the
//! Linux CI build compiling and lets the frontend hide the mic button without
//! a platform check of its own.

use serde::Serialize;
use tauri::AppHandle;
// Only the (Apple-gated) emitters below need the trait in scope.
#[cfg(any(target_os = "macos", target_os = "ios"))]
use tauri::Emitter;

use crate::error::Result;

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple;

/// A TCC (privacy) permission state, for either the microphone or speech
/// recognition. Mirrors both `SFSpeechRecognizerAuthorizationStatus` and
/// `AVAuthorizationStatus`, which share these four cases.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
// The full set is the wire contract on every platform, but only the Apple
// implementation ever reports anything other than `NotDetermined`.
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub enum Auth {
    /// Never asked — starting a session will prompt.
    NotDetermined,
    Authorized,
    /// The user said no; only System Settings can undo it.
    Denied,
    /// Blocked by policy (MDM, Screen Time), so prompting can't help.
    Restricted,
}

/// What dictation can do on this machine, for a UI that wants to disable the
/// mic button (or explain why) before the user ever presses it.
#[derive(Clone, Debug, Serialize)]
pub struct Availability {
    /// False on non-Apple platforms — nothing else in this struct matters.
    supported: bool,
    speech: Auth,
    microphone: Auth,
    /// The recognizer can transcribe without a network round-trip. When true
    /// we pin the request on-device, so no audio leaves the machine; when
    /// false recognition is server-backed and capped at about a minute.
    on_device: bool,
}

// The event machinery below is gated on the platforms that have an
// implementation: nothing emits without one, and CI builds Linux with
// `-D warnings`, where an unused payload type is a hard error.

/// A revision of the session's transcript. `text` is the entire utterance so
/// far, not an increment. At most one event per session has `is_final`: a
/// recognizer that never flushes gets torn down on the deadline instead.
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[derive(Clone, Serialize)]
struct TranscriptPayload {
    text: String,
    is_final: bool,
}

/// The lifecycle of the one live session. `Stopped` and `Error` are both
/// terminal *and* mean the session is fully torn down, so `dictation_start`
/// is callable again the moment either arrives.
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Listening,
    Stopped,
    Error,
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[derive(Clone, Serialize)]
struct StatePayload {
    state: State,
    /// Set only for `Error` — the recognizer's own message, shown as-is.
    error: Option<String>,
}

/// Emit one event, logging (not propagating) failure — same posture as
/// `supervisor::events`: no event is delivery-guaranteed.
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn emit<T: Serialize + Clone>(app: &AppHandle, event: &str, payload: T) {
    if let Err(e) = app.emit(event, payload) {
        tracing::warn!(error = %e, event, "emit failed");
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn emit_transcript(app: &AppHandle, text: String, is_final: bool) {
    emit(
        app,
        "dictation:transcript",
        TranscriptPayload { text, is_final },
    );
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn emit_state(app: &AppHandle, state: State, error: Option<String>) {
    emit(app, "dictation:state", StatePayload { state, error });
}

/// Whether dictation works here, and what permissions stand in the way. Cheap
/// and side-effect free — it never prompts, so the UI can call it on mount.
#[tauri::command]
pub async fn dictation_availability() -> Availability {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        apple::availability()
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        Availability {
            supported: false,
            speech: Auth::NotDetermined,
            microphone: Auth::NotDetermined,
            on_device: false,
        }
    }
}

/// Start listening. Requests microphone and speech permission on first use
/// (so the first call can block on two TCC prompts), and resolves once audio
/// is actually flowing — by which point `dictation:state` `listening` has
/// been emitted. A second call while a session is live is a no-op.
#[tauri::command]
pub async fn dictation_start(app: AppHandle) -> Result<()> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        apple::start(app).await
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        let _ = app;
        Err(crate::error::Error::Other(
            "dictation isn't supported on this platform".into(),
        ))
    }
}

/// Stop listening. Returns as soon as the microphone is released; the final
/// transcript and the terminal `dictation:state` follow asynchronously once
/// the recognizer has flushed, and a recognizer that doesn't flush in time
/// yields the terminal state alone. A no-op when idle, except that a stop
/// issued while `dictation_start` waits on a permission prompt cancels that
/// pending session.
#[tauri::command]
pub async fn dictation_stop(app: AppHandle) -> Result<()> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        apple::stop(app).await
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        let _ = app;
        Ok(())
    }
}
