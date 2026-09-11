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
//! There are two engines behind these commands, chosen per session by
//! [`engine`]: Apple's on-device `SpeechAnalyzer` (macOS 26+), which streams
//! revisions while the user speaks, and local whisper.cpp, which has nothing
//! to say until the mic closes and so spends the gap in `transcribing`. The
//! commands, the events and the session-id contract are identical either way
//! — `apple` owns the microphone for both.
//!
//! Only macOS has an implementation (`apple`); everywhere else the commands
//! are stubs that report `supported: false`, which is what keeps the Linux CI
//! build compiling and lets the frontend hide the mic button without a
//! platform check of its own. A Mac below 26 with the local engine off reports
//! the same, from the real implementation.

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::database;
use crate::error::Result;
use crate::DbState;

#[cfg(target_os = "macos")]
mod apple;
// The local engine's sink for the shared mic tap.
#[cfg(target_os = "macos")]
mod capture;
// The mic's loudness, for the composer's level bars and for the pause that
// ends a session.
#[cfg(target_os = "macos")]
mod level;
// The default engine's `SpeechAnalyzer` bridge (Rust side of
// `swift/SpeechBridge.swift`).
#[cfg(target_os = "macos")]
mod speech;
// Compiles everywhere (the catalog and download are plain Rust); the engine
// itself is gated inside. `pub` so `lib.rs` can seed the models root.
pub mod whisper;
// A paired phone's dictation: it captures, this Mac transcribes. Compiles
// everywhere so the remote dispatcher can name the ops; the transcription
// itself is macOS-only inside, like the engine.
pub mod remote;

/// How much audio one session may capture, whichever microphone it comes from.
/// A dictation is a sentence or two; this is the bound that keeps a mic left
/// open by a forgotten window (or a phone that stopped talking to us) from
/// growing the buffer — and the transcription that follows — without limit.
/// Audio past it is dropped, which truncates the transcript rather than failing.
pub(super) const MAX_CAPTURE_SECS: f64 = 300.0;

/// A TCC (privacy) permission state for the microphone. Mirrors
/// `AVAuthorizationStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
// The full set is the wire contract on every platform, but only the macOS
// implementation ever reports anything other than `NotDetermined`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum Auth {
    /// Never asked — starting a session will prompt.
    NotDetermined,
    Authorized,
    /// The user said no; only System Settings can undo it.
    Denied,
    /// Blocked by policy (MDM, Screen Time), so prompting can't help.
    Restricted,
}

/// Which recognizer a session would use. Reported by `dictation_availability`
/// so the UI knows, among other things, which engine stops itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
// Both variants are the wire contract everywhere, but `Whisper` is only ever
// chosen where whisper.cpp is built.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum Engine {
    /// Apple's on-device `SpeechAnalyzer` (macOS 26+), the default.
    Apple,
    /// Local whisper.cpp (see [`whisper`]).
    Whisper,
}

/// What dictation can do on this machine, for a UI that wants to disable the
/// mic button (or explain why) before the user ever presses it.
#[derive(Clone, Debug, Serialize)]
pub struct Availability {
    /// False where no engine can run — non-Apple platforms, and a Mac below
    /// macOS 26 (or with a language Apple has no model for) unless the local
    /// engine is chosen. Nothing else in this struct matters then.
    supported: bool,
    /// Vestigial: no engine asks for the Speech Recognition grant any more —
    /// Apple's analyzer is on-device and needs none — so this is always
    /// `NotDetermined`. Kept so the wire shape is unchanged.
    speech: Auth,
    microphone: Auth,
    /// Always true: both engines transcribe on this machine and no audio
    /// leaves it.
    on_device: bool,
    engine: Engine,
}

// The event machinery below is gated on the platforms that have an
// implementation: nothing emits without one, and CI builds Linux with
// `-D warnings`, where an unused payload type is a hard error.

/// A revision of the session's transcript. `text` is the entire utterance so
/// far, not an increment. At most one event per session has `is_final`: a
/// recognizer that never flushes gets torn down on the deadline instead.
///
/// `session` is the id `dictation_start` returned for this session. Events are
/// app-wide, and a session outlives the composer that started it (a stop is
/// followed by a flush), so a composer that starts the next one needs the id to
/// tell the old session's stragglers from its own.
#[cfg(target_os = "macos")]
#[derive(Clone, Serialize)]
struct TranscriptPayload {
    session: u64,
    text: String,
    is_final: bool,
}

/// The lifecycle of the one live session. `Stopped` and `Error` are both
/// terminal *and* mean the session is fully torn down, so `dictation_start`
/// is callable again the moment either arrives.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Listening,
    /// The mic is closed and the local engine is running the model. Only the
    /// whisper path emits this, between `listening` and the terminal state:
    /// the session is still alive and its final transcript is still coming.
    Transcribing,
    Stopped,
    Error,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Serialize)]
struct StatePayload {
    /// Same id as on [`TranscriptPayload`].
    session: u64,
    state: State,
    /// Set only for `Error` — the recognizer's own message, shown as-is.
    error: Option<String>,
}

/// How loud the mic is right now. Display only: the composer's level bars.
/// Emitted every `level::LEVEL_POLL` from the moment audio flows until the mic
/// closes, then never again for that session.
#[cfg(target_os = "macos")]
#[derive(Clone, Serialize)]
struct LevelPayload {
    /// Same id as on [`TranscriptPayload`].
    session: u64,
    /// 0 (silence) to 1 (loud speech) — see `level::normalize`.
    level: f32,
}

/// Emit one event, logging (not propagating) failure — same posture as
/// `supervisor::events`: no event is delivery-guaranteed.
fn emit<T: Serialize + Clone>(app: &AppHandle, event: &str, payload: T) {
    if let Err(e) = app.emit(event, payload) {
        tracing::warn!(error = %e, event, "emit failed");
    }
}

#[cfg(target_os = "macos")]
fn emit_level(app: &AppHandle, session: u64, level: f32) {
    emit(app, "dictation:level", LevelPayload { session, level });
}

#[cfg(target_os = "macos")]
fn emit_transcript(app: &AppHandle, session: u64, text: String, is_final: bool) {
    emit(
        app,
        "dictation:transcript",
        TranscriptPayload {
            session,
            text,
            is_final,
        },
    );
}

#[cfg(target_os = "macos")]
fn emit_state(app: &AppHandle, session: u64, state: State, error: Option<String>) {
    emit(
        app,
        "dictation:state",
        StatePayload {
            session,
            state,
            error,
        },
    );
}

/// The local engine's opt-in state, the model it is set to use, and the
/// catalog to choose from. Separate from [`Availability`], which describes the
/// platform recognizer: this is the Settings section's contract, and both
/// sides can be true at once (the engine is chosen, the weights are still
/// downloading).
#[derive(Clone, Serialize)]
pub struct ModelStatus {
    /// The `dictation_engine` setting selects the local engine.
    enabled: bool,
    /// The selected model's weights are on disk and verified, so the engine
    /// can load them.
    installed: bool,
    /// A download is running in this process; progress arrives as
    /// `dictation:model_progress`. Process-wide, so it can be a model other
    /// than the selected one — [`ModelStatus::downloading_id`] says which.
    downloading: bool,
    downloading_id: Option<&'static str>,
    /// The `dictation_model` selection, or the platform default until the user
    /// makes one.
    model: ModelInfo,
    /// Every catalog entry, so Settings can offer the choice without a second
    /// command.
    models: Vec<ModelInfo>,
}

/// One catalog entry as Settings describes it. `size` is the exact byte count,
/// so every size string in the UI is derived rather than pinned in two places.
#[derive(Clone, Serialize)]
pub struct ModelInfo {
    id: &'static str,
    label: &'static str,
    note: &'static str,
    size: u64,
    /// This entry's weights are on disk. Per-entry because a model the user
    /// switched away from stays downloaded until it is removed explicitly.
    installed: bool,
}

fn model_info(model: &'static whisper::models::WhisperModel) -> ModelInfo {
    ModelInfo {
        id: model.id,
        label: model.label,
        note: model.note,
        size: model.size,
        installed: whisper::models::installed_path(model).is_some(),
    }
}

fn model_status(
    enabled: bool,
    selected: &'static whisper::models::WhisperModel,
    install: whisper::install::Status,
) -> ModelStatus {
    ModelStatus {
        enabled,
        installed: install.installed,
        downloading: install.downloading,
        downloading_id: install.downloading_id,
        model: model_info(selected),
        models: whisper::models::MODELS.iter().map(model_info).collect(),
    }
}

/// The opt-in and the chosen model, read under one lock — the two settings
/// always travel together, and the connection isn't reentrant.
fn engine_settings(
    state: &tauri::State<'_, DbState>,
) -> (bool, &'static whisper::models::WhisperModel) {
    let conn = state.lock();
    let enabled =
        whisper::parse_enabled(database::get_setting(&conn, whisper::ENGINE_SETTING).as_deref());
    (enabled, whisper::selected(&conn))
}

/// Where the local engine stands: the opt-in, the choice, and what each
/// candidate's weights are doing. Cheap — a metadata stat per entry, no hashing
/// — so the Settings pane calls it on mount and after every action.
#[tauri::command]
pub fn dictation_model_status(state: tauri::State<'_, DbState>) -> ModelStatus {
    let (enabled, model) = engine_settings(&state);
    model_status(enabled, model, whisper::install::status(model))
}

/// Settings key: end a session on its own after a pause. Opt-out — only
/// `"false"` turns it off. Both engines, because the pause is measured off the
/// shared level meter rather than off either recognizer (see
/// `apple::watch_for_silence`). Mirrored in memory because the silence monitor
/// polls it off the audio thread, where there is no DB handle.
pub const AUTO_STOP_SETTING: &str = "dictation_auto_stop";
static AUTO_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn parse_auto_stop(raw: Option<&str>) -> bool {
    raw != Some("false")
}

pub fn set_auto_stop(enabled: bool) {
    AUTO_STOP.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

/// Read by the silence monitor on macOS, and on every platform by
/// [`remote::status`] — the phone hears its own pause, so it has to be told
/// what this Mac wants done about one. That second caller is why there is no
/// `allow(dead_code)` here any more.
pub(crate) fn auto_stop() -> bool {
    AUTO_STOP.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether a dictation session ends itself after a pause (Settings ›
/// Dictation). Persist-then-mirror, like `set_dictation_engine`.
#[tauri::command]
pub fn set_dictation_auto_stop(enabled: bool, state: tauri::State<'_, DbState>) -> Result<()> {
    {
        let conn = state.lock();
        database::set_setting(
            &conn,
            AUTO_STOP_SETTING,
            if enabled { "true" } else { "false" },
        )?;
    }
    set_auto_stop(enabled);
    Ok(())
}

/// Choose the dictation engine. Persists `dictation_engine` (backend-owned
/// snake_case key, so the renderer reads it as `s.dictation_engine`) and, when
/// enabling without the weights on disk, kicks the download off in the
/// background — the toggle can't await half a gigabyte. Same persist-then-act
/// shape as `set_code_indexing_enabled`.
///
/// Turning it off leaves the weights alone; removing them is a separate,
/// explicit action.
#[tauri::command]
pub fn set_dictation_engine(
    enabled: bool,
    app: AppHandle,
    state: tauri::State<'_, DbState>,
) -> Result<()> {
    let model = {
        let conn = state.lock();
        database::set_setting(
            &conn,
            whisper::ENGINE_SETTING,
            if enabled {
                whisper::ENGINE_WHISPER
            } else {
                whisper::ENGINE_APPLE
            },
        )?;
        whisper::selected(&conn)
    };
    if enabled {
        whisper::install::download(app, model);
    }
    Ok(())
}

/// Choose which catalog entry the local engine uses. Persists
/// `dictation_model` and, when the engine is on and the new choice isn't
/// downloaded, starts fetching it in the background — same shape as
/// [`set_dictation_engine`], because the choice can't await half a gigabyte
/// either.
///
/// The model being switched away from is left on disk: it is already paid for,
/// and switching back shouldn't cost the download twice. Removing it is a
/// separate, explicit action.
#[tauri::command]
pub fn set_dictation_model(
    id: String,
    app: AppHandle,
    state: tauri::State<'_, DbState>,
) -> Result<ModelStatus> {
    let model = catalog_entry(&id)?;
    let enabled = {
        let conn = state.lock();
        database::set_setting(&conn, whisper::MODEL_SETTING, model.id)?;
        whisper::parse_enabled(database::get_setting(&conn, whisper::ENGINE_SETTING).as_deref())
    };
    let install = if enabled {
        whisper::install::download(app, model)
    } else {
        whisper::install::status(model)
    };
    Ok(model_status(enabled, model, install))
}

/// Retry a failed or never-started download of the selected model. Returns
/// immediately with the state the call left things in; an already-installed
/// model, or any download already running, makes it a no-op.
#[tauri::command]
pub fn dictation_model_download(app: AppHandle, state: tauri::State<'_, DbState>) -> ModelStatus {
    let (enabled, model) = engine_settings(&state);
    let install = whisper::install::download(app, model);
    model_status(enabled, model, install)
}

/// Delete a model's downloaded weights — the selected one unless `id` names
/// another, since a model switched away from stays on disk and Settings is the
/// only place to reclaim it. When removing the selected model, the caller is
/// expected to turn the engine off first: an enabled engine with no model would
/// fall back to the platform recognizer, but silently.
#[tauri::command]
pub fn dictation_model_remove(
    id: Option<String>,
    state: tauri::State<'_, DbState>,
) -> Result<ModelStatus> {
    let (enabled, selected) = engine_settings(&state);
    let target = match &id {
        Some(id) => catalog_entry(id)?,
        None => selected,
    };
    whisper::install::remove(target);
    Ok(model_status(
        enabled,
        selected,
        whisper::install::status(selected),
    ))
}

/// An id from the frontend is only ever one the catalog handed it, so an
/// unknown one is a bug rather than a state to fall back from — silently
/// acting on the selected model instead would download or delete the wrong
/// weights.
fn catalog_entry(id: &str) -> Result<&'static whisper::models::WhisperModel> {
    whisper::models::find(id)
        .ok_or_else(|| crate::error::Error::Other(format!("unknown dictation model: {id}")))
}

/// Which engine a session started right now would use. The local one takes
/// both the opt-in AND a fully downloaded model: dispatching on the setting
/// alone would let an interrupted download leave the mic button dead.
#[cfg(target_os = "macos")]
fn engine(app: &AppHandle) -> Engine {
    use tauri::Manager;

    let (enabled, model) = engine_settings(&app.state::<DbState>());
    if enabled && whisper::models::installed_path(model).is_some() {
        Engine::Whisper
    } else {
        Engine::Apple
    }
}

/// Whether dictation works here, and what permissions stand in the way. Cheap
/// and side-effect free — it never prompts and never downloads a model, so the
/// UI can call it on mount.
#[tauri::command]
pub async fn dictation_availability(app: AppHandle) -> Availability {
    #[cfg(target_os = "macos")]
    {
        apple::availability(engine(&app)).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Availability {
            supported: false,
            speech: Auth::NotDetermined,
            microphone: Auth::NotDetermined,
            on_device: false,
            engine: Engine::Apple,
        }
    }
}

/// Start listening. Requests the microphone permission on first use (so the
/// first call can block on a TCC prompt), makes sure Apple's engine has its
/// speech model for this Mac's language (a missing one starts downloading and
/// rejects with a readable message; try again once it has landed), and
/// resolves once audio is actually flowing.
///
/// `Some(id)` means a session is now live and `dictation:state` `listening` has
/// been emitted, so a terminal state will follow; every event of that session
/// carries the same `id`, which is how a caller tells its own session from a
/// previous one still flushing. `None` means nothing was started and no event
/// will arrive: either a session was already active (a second start is a
/// no-op) or a `dictation_stop` issued while the prompt was up cancelled this
/// one. The caller needs the distinction because `None` leaves nothing to
/// wait for.
#[tauri::command]
pub async fn dictation_start(app: AppHandle) -> Result<Option<u64>> {
    #[cfg(target_os = "macos")]
    {
        let engine = engine(&app);
        apple::start(app, engine).await
    }
    #[cfg(not(target_os = "macos"))]
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
/// yields the terminal state alone. The local engine takes the same shape,
/// with a `transcribing` state for the gap while the model runs. A no-op when
/// idle, except that a stop issued while `dictation_start` waits on a
/// permission prompt cancels that pending session.
#[tauri::command]
pub async fn dictation_stop(app: AppHandle) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        apple::stop(app).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(())
    }
}
