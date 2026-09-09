//! The microphone half of dictation: an `AVAudioEngine` input tap, and the two
//! places its audio can go.
//!
//! Both engines share everything about opening the mic and owning the one live
//! session; they differ only in the [`Sink`] the tap feeds. Apple's
//! `SpeechAnalyzer` (behind [`super::speech`] and its Swift bridge) streams
//! results back on its own, which is why a stop there hands off to the
//! analyzer's flush. The local engine's sink is a plain PCM buffer
//! (`super::capture`) that is transcribed in one pass once the mic is closed.
//!
//! # Threading
//!
//! AVFoundation objects are not `Send`, and both Apple and the Swift bridge
//! call us back on threads of their own choosing. Rather than wrap the handles
//! in a `Send` lie, everything that touches session state runs on the main
//! thread ([`on_main`], [`spawn_main`]), and the session itself lives in a
//! `thread_local` — so there is no lock to hold, nothing to declare
//! `unsafe impl Send`, and no way for a callback to observe a half-built
//! session.
//!
//! The bridge's callbacks arrive on Swift's executors, never on the main
//! thread, so each one hops onto it with `run_on_main_thread`. The hop is a
//! dispatch, not a re-entry: a callback cannot run before the `on_main` block
//! that created its session has returned. The one exception is the audio tap,
//! which Apple invokes on a real-time render thread — it deliberately touches
//! no session state, only its own sink handle, which is the pattern Apple
//! documents for it.
//!
//! Note that `#[tauri::command]` futures must be `Send`, which is the second
//! reason for this shape: no ObjC handle is ever live across an `.await`.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_avf_audio::{
    AVAudioEngine, AVAudioFormat, AVAudioInputNode, AVAudioPCMBuffer, AVAudioTime,
};
use tauri::AppHandle;

use super::level::{self, Meter};
use super::{emit_state, emit_transcript, speech, Auth, Availability, Engine, State};
use crate::error::{Error, Result};

/// The engine's only input bus.
const BUS: usize = 0;

/// Tap buffer size in sample frames, matching Apple's own live-recognition
/// sample. The engine clamps this to a size it can service.
const TAP_BUFFER_FRAMES: u32 = 1024;

/// How long to wait after the input ends for the analyzer's final result
/// before forcing the teardown. It normally finalizes in well under a second;
/// the deadline exists so a wedged analyzer can't leave the UI stuck in
/// `listening` with no way back.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// The tap block Apple invokes on the render thread. Owned by the session for
/// as long as the tap is installed.
pub(super) type Tap = RcBlock<dyn Fn(NonNull<AVAudioPCMBuffer>, NonNull<AVAudioTime>)>;

thread_local! {
    /// The one live session, main-thread only. See the module's threading note.
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };

    /// A `dictation_stop` that arrived while a start was in flight — `ACTIVE`
    /// claimed but `SESSION` still empty, which is exactly the span of the
    /// permission prompt and the model check. Without this the stop would be
    /// swallowed and the mic would open anyway once the user granted access.
    /// Set only in that window (an idle stop leaves nothing behind), consumed
    /// by `begin`, and cleared on the one path that abandons a claim without
    /// reaching `begin`. Both ends run on the main thread, which is what orders
    /// a stop against a concurrent start.
    static STOP_PENDING: Cell<bool> = const { Cell::new(false) };
}

/// Set for the whole span of a session, from the first moment of `start`
/// until teardown finishes. Unlike `SESSION` this is readable from any
/// thread, which is what lets `start` reject a concurrent second start
/// without a main-thread hop, and makes "a second start while listening is a
/// no-op" hold even against two commands racing.
static ACTIVE: AtomicBool = AtomicBool::new(false);

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Where a session's audio goes. Both variants clone cheaply, so `stop` can
/// lift the sink out of `SESSION` before calling into the frameworks.
#[derive(Clone)]
enum Sink {
    /// Apple's analyzer, which delivers its own results through the bridge's
    /// callbacks and needs `finish` to flush the last of them.
    Speech(speech::Session),
    /// The local engine's capture buffer, transcribed in one pass at stop.
    Pcm(std::sync::Arc<super::capture::Pcm>),
}

impl Sink {
    /// Drop the sink's work without waiting for a result — for a session that
    /// failed to come up, or one being torn down. The local engine's buffer is
    /// simply dropped; marking it closed is what retires its silence monitor.
    fn cancel(&self) {
        match self {
            Sink::Speech(session) => session.cancel(),
            Sink::Pcm(pcm) => pcm.close(),
        }
    }
}

struct Session {
    /// Distinguishes this session from its successors. Every callback that may
    /// arrive late (a post-cancel result, the flush deadline) carries the
    /// generation it was created for and does nothing if it no longer matches
    /// — otherwise a straggler could tear down a session the user has since
    /// started.
    generation: u64,
    audio: Retained<AVAudioEngine>,
    input: Retained<AVAudioInputNode>,
    sink: Sink,
    /// The mic's loudness, fed by the tap whichever sink it has, and read by
    /// the `dictation:level` emitter until the mic closes.
    meter: Arc<Meter>,
    /// Kept alive for the tap's lifetime. `installTapOnBus` is documented to
    /// take ownership, but holding our own reference costs nothing and takes
    /// a use-after-free off the table.
    _tap: Tap,
    /// True once the user asked to stop. An analyzer error after that point
    /// (a cancellation racing the finalization) is the tail of a normal
    /// session and must surface as `stopped`, not `error`.
    stopping: bool,
}

/// What a stop left for the caller to finish once it is off the main thread.
enum Stopping {
    /// Apple's analyzer owns the rest: ending the input makes it flush a final
    /// result, and that result drives the teardown and the terminal state.
    Flushing(u64),
    /// The local engine: the mic is already closed and the session claimed, and
    /// the audio is the caller's to transcribe.
    Captured(u64, std::sync::Arc<super::capture::Pcm>),
}

// ---------------------------------------------------------------------------
// Main-thread plumbing

/// Run `f` on the main thread and wait for its value. Every call that touches
/// `SESSION` or a session's ObjC handles goes through here.
async fn on_main<T: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| Error::Other(format!("dictation: main thread unavailable: {e}")))?;
    rx.await
        .map_err(|_| Error::Other("dictation: main thread task was dropped".into()))
}

/// Fire-and-forget [`on_main`], for callbacks that have no one to answer to.
/// The only failure is an app that is shutting down, which is logged, not
/// propagated — there is no session left to matter.
fn spawn_main(app: &AppHandle, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = app.run_on_main_thread(f) {
        tracing::warn!(error = %e, "dictation: main thread unavailable");
    }
}

/// Is the session that `generation` belongs to still the live one?
fn is_live(generation: u64) -> bool {
    SESSION.with_borrow(|s| s.as_ref().is_some_and(|s| s.generation == generation))
}

/// The live session's capture buffer, if `generation` is still it and it is a
/// local-engine session. Main thread, like every read of `SESSION`; the buffer
/// itself is then readable from anywhere.
fn captured(generation: u64) -> Option<std::sync::Arc<super::capture::Pcm>> {
    SESSION.with_borrow(|slot| {
        let session = slot.as_ref().filter(|s| s.generation == generation)?;
        match &session.sink {
            Sink::Pcm(pcm) => Some(pcm.clone()),
            Sink::Speech(_) => None,
        }
    })
}

/// Claim the live session, but only if it is still `generation`. Returning
/// the `Session` by value makes teardown exactly-once: whoever gets it owns
/// the terminal `dictation:state` emit.
fn claim(generation: u64) -> Option<Session> {
    SESSION.with_borrow_mut(|slot| {
        if slot.as_ref().is_some_and(|s| s.generation == generation) {
            slot.take()
        } else {
            None
        }
    })
}

/// Release the microphone and the sink, and let a new session start.
fn teardown(session: Session) {
    unsafe {
        session.audio.stop();
        session.input.removeTapOnBus(BUS);
    }
    session.sink.cancel();
    session.meter.close();
    ACTIVE.store(false, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Permissions

impl From<AVAuthorizationStatus> for Auth {
    fn from(status: AVAuthorizationStatus) -> Self {
        match status {
            AVAuthorizationStatus::Authorized => Auth::Authorized,
            AVAuthorizationStatus::Denied => Auth::Denied,
            AVAuthorizationStatus::Restricted => Auth::Restricted,
            // NotDetermined, and anything a future OS adds: treat as "ask".
            _ => Auth::NotDetermined,
        }
    }
}

/// TCC's microphone status. `AVMediaTypeAudio` is an `extern` string constant,
/// so it is nil only if AVFoundation failed to load at all; treat that as
/// "can't tell, ask".
fn microphone_auth() -> Auth {
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return Auth::NotDetermined;
    };
    unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) }.into()
}

/// Turn a settled permission state into a message the user can act on. The
/// prompt only ever appears once, so a denied permission is only fixable in
/// System Settings — say so rather than silently doing nothing.
fn authorized_or_error(what: &str, auth: Auth) -> Result<()> {
    match auth {
        Auth::Authorized => Ok(()),
        Auth::Denied | Auth::NotDetermined => Err(Error::Other(format!(
            "Fletch needs {what} access to dictate. Enable it in System Settings > \
             Privacy & Security."
        ))),
        Auth::Restricted => Err(Error::Other(format!(
            "{what} access is restricted on this device, so dictation can't run."
        ))),
    }
}

/// `requestAccessForMediaType:` hands its answer to a block on an arbitrary
/// queue. Deliberately non-`async`: the block is created, handed off, and
/// dropped entirely within the call, so no ObjC handle is ever live across the
/// caller's `.await`. Apple copies the block, so releasing our reference here
/// is safe.
fn request_microphone_auth() -> tokio::sync::oneshot::Receiver<Auth> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return rx;
    };
    let tx = parking_lot::Mutex::new(Some(tx));
    let handler = RcBlock::new(move |granted: objc2::runtime::Bool| {
        if let Some(tx) = tx.lock().take() {
            let _ = tx.send(if granted.as_bool() {
                Auth::Authorized
            } else {
                Auth::Denied
            });
        }
    });
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &handler) };
    rx
}

/// Ensure the microphone permission, prompting if it hasn't been asked yet.
/// The only grant either engine needs: Apple's analyzer runs on-device and
/// asks for no Speech Recognition authorization (verified against a fresh
/// bundle id — no prompt, no usage string, status stays "not determined").
async fn ensure_authorized() -> Result<()> {
    let mut mic = microphone_auth();
    if mic == Auth::NotDetermined {
        mic = request_microphone_auth().await.map_err(|_| {
            Error::Other("dictation: microphone permission prompt was dismissed".into())
        })?;
    }
    authorized_or_error("microphone", mic)
}

/// Everything a session needs settled before the mic opens: the permission,
/// and for Apple's engine the locale, model assets and audio format. Both can
/// take a while (a TCC prompt; a model download) and both reject with a message
/// the user can act on.
async fn ready(engine: Engine) -> Result<()> {
    ensure_authorized().await?;
    if engine == Engine::Apple {
        speech::prepare().await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands

pub async fn availability(engine: Engine) -> Availability {
    // The local engine runs wherever whisper.cpp is built; Apple's needs
    // macOS 26 and a model for this Mac's language. Neither sends audio off
    // the machine, and neither asks for the speech-recognition grant.
    let supported = engine == Engine::Whisper || speech::supported().await;
    Availability {
        supported,
        speech: Auth::NotDetermined,
        microphone: microphone_auth(),
        on_device: true,
        engine,
    }
}

/// `Ok(Some(id))` once a session is live and `listening` has been emitted —
/// the id every event of that session carries; `Ok(None)` when nothing was
/// started, so no terminal event will follow.
pub async fn start(app: AppHandle, engine: Engine) -> Result<Option<u64>> {
    // Claiming ACTIVE up front is what makes a second start a no-op, and it
    // has to happen before the permission prompt, which can sit on screen
    // for a long time.
    if ACTIVE.swap(true, Ordering::SeqCst) {
        return Ok(None);
    }
    // The claim is released by exactly one owner: `start` while `begin` still
    // hasn't run, then `begin` itself on failure, then `teardown` once the
    // session is up. Handing it over rather than clearing it from here means
    // a caller who drops this future after the session came up can't leave a
    // live analyzer behind a cleared flag.
    if let Err(e) = ready(engine).await {
        ACTIVE.store(false, Ordering::SeqCst);
        // The only path that drops the claim without `begin` consuming a stop
        // that landed during the wait — clear it, or it would cancel an
        // unrelated later start.
        let _ = on_main(&app, || STOP_PENDING.set(false)).await;
        return Err(e);
    }
    let handle = app.clone();
    let started = match on_main(&app, move || begin(handle, engine)).await {
        Ok(started) => started?,
        // The closure never ran, so `begin` never took the claim.
        Err(e) => {
            ACTIVE.store(false, Ordering::SeqCst);
            return Err(e);
        }
    };
    let Some((generation, meter)) = started else {
        return Ok(None);
    };
    // The level bars run for every session; the meter closes with the mic.
    level::watch(app.clone(), generation, meter);
    // Hands-free stop is the local engine's alone: Apple's analyzer decides
    // for itself when an utterance has ended, and we have no PCM to measure.
    if engine == Engine::Whisper {
        watch_for_silence(app, generation);
    }
    Ok(Some(generation))
}

/// Poll a local-engine session's speech tracker and stop it once the user has
/// spoken and then gone quiet — the whole of hands-free dictation. Stops
/// through the same path a second click takes, so `transcribing`, the
/// transcript and `stopped` follow in the usual order.
///
/// Everything here is keyed on `generation`: the task exits as soon as the
/// buffer it was given is closed (any teardown does that), and the stop it
/// finally issues names its own session, so a monitor that outlives its
/// session cannot cut a later one short.
fn watch_for_silence(app: AppHandle, generation: u64) {
    tokio::spawn(async move {
        let Ok(Some(pcm)) = on_main(&app, move || captured(generation)).await else {
            return;
        };
        loop {
            tokio::time::sleep(super::capture::SILENCE_POLL).await;
            if pcm.is_closed() {
                return;
            }
            if pcm.done_talking() {
                break;
            }
        }
        if let Err(e) = stop_session(app, Some(generation)).await {
            tracing::warn!(error = %e, "dictation: silence auto-stop failed");
        }
    });
}

/// `Some((generation, meter))` for a session that came up: its id, and the
/// level meter its tap feeds, handed out so the emitter can be started off the
/// main thread.
fn begin(app: AppHandle, engine: Engine) -> Result<Option<(u64, Arc<Meter>)>> {
    // The user asked to stop while the permission prompt was up. Honour it
    // instead of opening the mic behind their back. No state event: the
    // session never came up, so there is nothing to close out — the `None`
    // is what tells the caller not to wait for one.
    if STOP_PENDING.replace(false) {
        ACTIVE.store(false, Ordering::SeqCst);
        return Ok(None);
    }
    match build_session(app, engine) {
        Ok(started) => Ok(Some(started)),
        Err(e) => {
            // `build_session` unwinds whatever it installed, so releasing the
            // claim here is what lets the user retry. No state event — the
            // command's `Err` is the frontend's signal.
            ACTIVE.store(false, Ordering::SeqCst);
            Err(e)
        }
    }
}

/// Build and start the session, returning its generation — the id the frontend
/// keys events on. Main thread; permissions are already granted, which matters
/// because reading `inputNode`'s format before that yields a zero-rate format
/// and installing a tap with it throws in ObjC.
fn build_session(app: AppHandle, engine: Engine) -> Result<(u64, Arc<Meter>)> {
    let (audio, input, format) = open_microphone()?;
    let generation = NEXT_GENERATION.fetch_add(1, Ordering::SeqCst);

    let meter = Meter::new(&format);
    let (sink, tap) = match engine {
        Engine::Apple => speech_sink(&app, generation, &format, meter.clone())?,
        Engine::Whisper => {
            let (pcm, tap) = super::capture::sink(&format, meter.clone())?;
            (Sink::Pcm(pcm), tap)
        }
    };

    unsafe {
        input.installTapOnBus_bufferSize_format_block(
            BUS,
            TAP_BUFFER_FRAMES,
            Some(&format),
            RcBlock::as_ptr(&tap),
        );
        audio.prepare();
        if let Err(e) = audio.startAndReturnError() {
            // Unwind what we just built rather than leaving a tap installed
            // and a task running behind a failed start.
            input.removeTapOnBus(BUS);
            sink.cancel();
            return Err(Error::Other(format!(
                "Couldn't start the microphone: {}",
                e.localizedDescription()
            )));
        }
    }

    SESSION.set(Some(Session {
        generation,
        audio,
        input,
        sink,
        meter: meter.clone(),
        _tap: tap,
        stopping: false,
    }));
    // Audio is flowing. Only now can the frontend show a live mic.
    emit_state(&app, generation, State::Listening, None);
    Ok((generation, meter))
}

/// The input node and the format its tap will deliver.
fn open_microphone() -> Result<(
    Retained<AVAudioEngine>,
    Retained<AVAudioInputNode>,
    Retained<AVAudioFormat>,
)> {
    let audio = unsafe { AVAudioEngine::new() };
    let input = unsafe { audio.inputNode() };
    let format = unsafe { input.outputFormatForBus(BUS) };
    // A zero-rate or channel-less format means the OS gave us no usable input
    // device. Installing a tap with it raises an ObjC exception, which would
    // abort the process rather than surface an error, so check first.
    if unsafe { format.sampleRate() } <= 0.0 || unsafe { format.channelCount() } == 0 {
        return Err(Error::Other(
            "No microphone input is available. Check your input device in System Settings > Sound."
                .into(),
        ));
    }
    Ok((audio, input, format))
}

/// Apple's analyzer, plus the tap that streams the mic straight into it. The
/// format conversion the analyzer needs happens in the bridge, not here.
fn speech_sink(
    app: &AppHandle,
    generation: u64,
    format: &AVAudioFormat,
    meter: Arc<Meter>,
) -> Result<(Sink, Tap)> {
    let session = speech::Session::start(
        format,
        speech::Events {
            on_result: Box::new(on_result(app.clone(), generation)),
            on_error: Box::new(on_error(app.clone(), generation)),
        },
    )?;
    let tap = RcBlock::new(
        move |buffer: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
            // Real-time audio thread. Measuring the buffer (one pass, one
            // atomic store) and handing it over are the only things that may
            // happen here — no session state, no emit.
            let buffer = unsafe { buffer.as_ref() };
            meter.record_buffer(buffer);
            session.feed(buffer);
        },
    );
    Ok((Sink::Speech(session), tap))
}

/// The analyzer's transcript callback: every revision of the running text, and
/// once, last, the final one that ends the session.
fn on_result(app: AppHandle, generation: u64) -> impl Fn(String, bool) + Send + Sync {
    move |text, is_final| {
        let emit_to = app.clone();
        spawn_main(&app, move || {
            // A result for an already-replaced session must stay silent, or it
            // would overwrite the new session's transcript.
            if !is_live(generation) {
                return;
            }
            emit_transcript(&emit_to, generation, text, is_final);
            if is_final {
                if let Some(session) = claim(generation) {
                    teardown(session);
                    emit_state(&emit_to, generation, State::Stopped, None);
                }
            }
        });
    }
}

/// The analyzer's failure callback. After a user's stop it is the expected
/// tail of the session; before one it is a real error.
fn on_error(app: AppHandle, generation: u64) -> impl Fn(String) + Send + Sync {
    move |message| {
        let emit_to = app.clone();
        spawn_main(&app, move || {
            let Some(session) = claim(generation) else {
                return;
            };
            let stopping = session.stopping;
            teardown(session);
            if stopping {
                tracing::debug!(error = %message, "dictation stream ended");
                emit_state(&emit_to, generation, State::Stopped, None);
            } else {
                tracing::warn!(error = %message, "dictation failed");
                emit_state(&emit_to, generation, State::Error, Some(message));
            }
        });
    }
}

pub async fn stop(app: AppHandle) -> Result<()> {
    stop_session(app, None).await
}

/// End the live session. `expect` is `None` for a user's stop — whatever is
/// listening — and `Some(generation)` for the silence monitor, which owns one
/// session and must be a no-op against any other.
async fn stop_session(app: AppHandle, expect: Option<u64>) -> Result<()> {
    let stopped = on_main(&app, move || {
        // Clone the handles out before calling into the frameworks: the rule
        // for `SESSION` is that no borrow is ever held across such a call.
        let live = SESSION.with_borrow_mut(|slot| {
            let session = slot.as_mut()?;
            if expect.is_some_and(|g| g != session.generation) {
                return None;
            }
            session.stopping = true;
            Some((
                session.generation,
                session.audio.clone(),
                session.input.clone(),
                session.sink.clone(),
                session.meter.clone(),
            ))
        });
        let Some((generation, audio, input, sink, meter)) = live else {
            // No session of ours — but a claimed `ACTIVE` with an empty
            // `SESSION` means a `start` is still sitting on the permission
            // prompt, so leave the request for `begin` to consume. Genuinely
            // idle, record nothing: a flag left behind here would cancel a
            // later start. A generation-scoped stop never leaves one: its own
            // session existed, so this is a session that has already ended, and
            // the pending start is somebody else's.
            if expect.is_none() && ACTIVE.load(Ordering::SeqCst) {
                STOP_PENDING.set(true);
            }
            return None;
        };
        // The mic goes quiet either way; what happens after depends on the sink.
        unsafe {
            audio.stop();
            input.removeTapOnBus(BUS);
        }
        // No more buffers, so no more levels — even while Apple's flush keeps
        // the session alive for its final result.
        meter.close();
        match sink {
            Sink::Speech(session) => {
                // Deliberately not `cancel`: ending the input is what makes
                // the analyzer finalize, and that final result is what drives
                // the `stopped` emit. Cancelling would discard it — so the
                // session stays in `SESSION` for the callback.
                session.finish();
                Some(Stopping::Flushing(generation))
            }
            Sink::Pcm(pcm) => {
                // Nothing will flush, so the session is over here. Claiming it
                // now gives the transcription sole ownership of the terminal
                // state, and releases the mic for the next start.
                if let Some(session) = claim(generation) {
                    teardown(session);
                }
                Some(Stopping::Captured(generation, pcm))
            }
        }
    })
    .await?;

    match stopped {
        // Idle: nothing to stop, and no event.
        None => Ok(()),
        Some(Stopping::Flushing(generation)) => {
            let handle = app.clone();
            tokio::spawn(async move {
                tokio::time::sleep(FLUSH_TIMEOUT).await;
                let emit_to = handle.clone();
                let _ = on_main(&handle, move || {
                    // Still live means the final result never arrived; `claim`
                    // is what keeps this from double-emitting against the
                    // result callback.
                    if let Some(session) = claim(generation) {
                        tracing::warn!(
                            "dictation: no final result before the flush deadline; forcing stop"
                        );
                        teardown(session);
                        emit_state(&emit_to, generation, State::Stopped, None);
                    }
                })
                .await;
            });
            Ok(())
        }
        Some(Stopping::Captured(generation, pcm)) => {
            // The mic is already closed, but the model still has to run, which
            // is seconds rather than milliseconds — the frontend gets a state
            // of its own for that wait instead of a mic that looks stuck.
            emit_state(&app, generation, State::Transcribing, None);
            // The model is read here, at the stop, so the choice a session
            // transcribes with is the one showing in Settings when it ended.
            let (_, model) = {
                use tauri::Manager;
                super::engine_settings(&app.state::<crate::DbState>())
            };
            tokio::spawn(async move {
                match super::capture::transcribe(pcm, model).await {
                    Ok(text) => {
                        // Empty means the clip had no speech in it (see the
                        // engine's silence gate); there is nothing to splice
                        // into the composer, but the session still ends.
                        if !text.is_empty() {
                            emit_transcript(&app, generation, text, true);
                        }
                        emit_state(&app, generation, State::Stopped, None);
                    }
                    Err(e) => {
                        let message = e.to_string();
                        tracing::warn!(error = %message, "dictation: transcription failed");
                        emit_state(&app, generation, State::Error, Some(message));
                    }
                }
            });
            Ok(())
        }
    }
}
