//! Dictation on Apple's Speech framework: an `AVAudioEngine` mic tap feeding
//! an `SFSpeechAudioBufferRecognitionRequest`.
//!
//! # Threading
//!
//! Speech and AVFoundation objects are not `Send`, and Apple calls our blocks
//! back on queues of its own choosing. Rather than wrap the handles in a
//! `Send` lie, everything that touches session state runs on the main thread
//! (`on_main`), and the session itself lives in a `thread_local` — so there is
//! no lock to hold, nothing to declare `unsafe impl Send`, and no way for a
//! callback to observe a half-built session.
//!
//! That works because `SFSpeechRecognizer`'s `queue` defaults to the main
//! queue: the result handler is *dispatched* to the same thread we mutate
//! state on rather than re-entering us, so it cannot run until the
//! `on_main` block that created the task has returned. The one exception is
//! the audio tap, which Apple invokes on a real-time render thread — it
//! deliberately touches no session state, only `appendAudioPCMBuffer` on its
//! own retained request, which is the pattern Apple documents for it.
//!
//! Note that `#[tauri::command]` futures must be `Send`, which is the second
//! reason for this shape: no ObjC handle is ever live across an `.await`.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::AllocAnyThread;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_avf_audio::{AVAudioEngine, AVAudioInputNode, AVAudioPCMBuffer, AVAudioTime};
use objc2_foundation::NSError;
use objc2_speech::{
    SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionResult, SFSpeechRecognitionTask,
    SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus,
};
use tauri::AppHandle;

use super::{emit_state, emit_transcript, Auth, Availability, State};
use crate::error::{Error, Result};

/// The engine's only input bus.
const BUS: usize = 0;

/// Tap buffer size in sample frames, matching Apple's own live-recognition
/// sample. The engine clamps this to a size it can service.
const TAP_BUFFER_FRAMES: u32 = 1024;

/// How long to wait after `endAudio` for the recognizer's final result before
/// forcing the teardown. Apple normally flushes in well under a second; the
/// deadline exists so a wedged recognizer can't leave the UI stuck in
/// `listening` with no way back.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

thread_local! {
    /// The one live session, main-thread only. See the module's threading note.
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };

    /// A `dictation_stop` that found no session to stop. `start` sits on the
    /// permission prompts with `ACTIVE` claimed but `SESSION` still empty, so
    /// without this a stop issued in that window would be swallowed and the
    /// mic would open anyway once the user granted access. `start` clears the
    /// flag before prompting and consumes it after; both ends run on the main
    /// thread, which is what orders a stop against a concurrent start.
    static STOP_PENDING: Cell<bool> = const { Cell::new(false) };
}

/// Set for the whole span of a session, from the first moment of `start`
/// until teardown finishes. Unlike `SESSION` this is readable from any
/// thread, which is what lets `start` reject a concurrent second start
/// without a main-thread hop, and makes "a second start while listening is a
/// no-op" hold even against two commands racing.
static ACTIVE: AtomicBool = AtomicBool::new(false);

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(0);

struct Session {
    /// Distinguishes this session from its successors. Every block Apple may
    /// invoke late (a post-cancel result, the flush deadline) carries the
    /// generation it was created for and does nothing if it no longer matches
    /// — otherwise a straggler could tear down a session the user has since
    /// started.
    generation: u64,
    engine: Retained<AVAudioEngine>,
    input: Retained<AVAudioInputNode>,
    request: Retained<SFSpeechAudioBufferRecognitionRequest>,
    task: Retained<SFSpeechRecognitionTask>,
    /// Kept alive for the tap's lifetime. `installTapOnBus` is documented to
    /// take ownership, but holding our own reference costs nothing and takes
    /// a use-after-free off the table.
    _tap: RcBlock<dyn Fn(NonNull<AVAudioPCMBuffer>, NonNull<AVAudioTime>)>,
    /// True once the user asked to stop. After that point the recognizer
    /// reports the end of the stream *as an error* (a cancellation or
    /// no-speech code from a private domain), which is the expected tail of a
    /// normal session and must surface as `stopped`, not `error`.
    stopping: bool,
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

/// Is the session that `generation` belongs to still the live one?
fn is_live(generation: u64) -> bool {
    SESSION.with_borrow(|s| s.as_ref().is_some_and(|s| s.generation == generation))
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

/// Release the microphone and the recognizer, and let a new session start.
fn teardown(session: Session) {
    unsafe {
        session.engine.stop();
        session.input.removeTapOnBus(BUS);
        session.task.cancel();
    }
    ACTIVE.store(false, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Permissions

impl From<SFSpeechRecognizerAuthorizationStatus> for Auth {
    fn from(status: SFSpeechRecognizerAuthorizationStatus) -> Self {
        match status {
            SFSpeechRecognizerAuthorizationStatus::Authorized => Auth::Authorized,
            SFSpeechRecognizerAuthorizationStatus::Denied => Auth::Denied,
            SFSpeechRecognizerAuthorizationStatus::Restricted => Auth::Restricted,
            // NotDetermined, and anything a future OS adds: treat as "ask".
            _ => Auth::NotDetermined,
        }
    }
}

impl From<AVAuthorizationStatus> for Auth {
    fn from(status: AVAuthorizationStatus) -> Self {
        match status {
            AVAuthorizationStatus::Authorized => Auth::Authorized,
            AVAuthorizationStatus::Denied => Auth::Denied,
            AVAuthorizationStatus::Restricted => Auth::Restricted,
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

fn speech_auth() -> Auth {
    unsafe { SFSpeechRecognizer::authorizationStatus() }.into()
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

/// The two `requestAuthorization`-style APIs hand their answer to a block on
/// an arbitrary queue. These helpers are deliberately non-`async`: the block
/// is created, handed off, and dropped entirely within them, so no ObjC handle
/// is ever live across the caller's `.await`. Apple copies the block, so
/// releasing our reference here is safe.
fn request_speech_auth() -> tokio::sync::oneshot::Receiver<Auth> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = parking_lot::Mutex::new(Some(tx));
    let handler = RcBlock::new(move |status: SFSpeechRecognizerAuthorizationStatus| {
        if let Some(tx) = tx.lock().take() {
            let _ = tx.send(status.into());
        }
    });
    unsafe { SFSpeechRecognizer::requestAuthorization(&handler) };
    rx
}

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

/// Ensure both permissions are granted, prompting for whichever hasn't been
/// asked yet. Sequential rather than concurrent so the user sees one dialog at
/// a time.
async fn ensure_authorized() -> Result<()> {
    let mut speech = speech_auth();
    if speech == Auth::NotDetermined {
        speech = request_speech_auth().await.map_err(|_| {
            Error::Other("dictation: speech permission prompt was dismissed".into())
        })?;
    }
    authorized_or_error("speech recognition", speech)?;

    let mut mic = microphone_auth();
    if mic == Auth::NotDetermined {
        mic = request_microphone_auth().await.map_err(|_| {
            Error::Other("dictation: microphone permission prompt was dismissed".into())
        })?;
    }
    authorized_or_error("microphone", mic)
}

// ---------------------------------------------------------------------------
// Commands

pub fn availability() -> Availability {
    // A recognizer only exists for a supported locale; without one there is
    // nothing to ask about on-device support, so report the conservative
    // answer and let `start` produce the readable error.
    let on_device = unsafe { SFSpeechRecognizer::init(SFSpeechRecognizer::alloc()) }
        .is_some_and(|r| unsafe { r.supportsOnDeviceRecognition() });
    Availability {
        supported: true,
        speech: speech_auth(),
        microphone: microphone_auth(),
        on_device,
    }
}

pub async fn start(app: AppHandle) -> Result<()> {
    // Claiming ACTIVE up front is what makes a second start a no-op, and it
    // has to happen before the permission prompts, which can sit on screen
    // for a long time.
    if ACTIVE.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    // Discard a stop that predates this start; only one that lands while the
    // prompts are up should cancel us.
    if let Err(e) = on_main(&app, || STOP_PENDING.set(false)).await {
        ACTIVE.store(false, Ordering::SeqCst);
        return Err(e);
    }
    // The claim is released by exactly one owner: `start` while `begin` still
    // hasn't run, then `begin` itself on failure, then `teardown` once the
    // session is up. Handing it over rather than clearing it from here means
    // a caller who drops this future after the session came up can't leave a
    // live recognizer behind a cleared flag.
    if let Err(e) = ensure_authorized().await {
        ACTIVE.store(false, Ordering::SeqCst);
        return Err(e);
    }
    let handle = app.clone();
    match on_main(&app, move || begin(handle)).await {
        Ok(started) => started,
        // The closure never ran, so `begin` never took the claim.
        Err(e) => {
            ACTIVE.store(false, Ordering::SeqCst);
            Err(e)
        }
    }
}

fn begin(app: AppHandle) -> Result<()> {
    // The user asked to stop while the permission prompts were up. Honour it
    // instead of opening the mic behind their back. No state event: the
    // session never came up, so there is nothing to close out.
    if STOP_PENDING.replace(false) {
        ACTIVE.store(false, Ordering::SeqCst);
        return Ok(());
    }
    let started = build_session(app);
    if started.is_err() {
        // `build_session` unwinds whatever it installed, so releasing the
        // claim here is what lets the user retry. No state event — the
        // command's `Err` is the frontend's signal.
        ACTIVE.store(false, Ordering::SeqCst);
    }
    started
}

/// Build and start the session. Main thread; permissions are already granted,
/// which matters because reading `inputNode`'s format before that yields a
/// zero-rate format and installing a tap with it throws in ObjC.
fn build_session(app: AppHandle) -> Result<()> {
    let recognizer = unsafe { SFSpeechRecognizer::init(SFSpeechRecognizer::alloc()) }
        .ok_or_else(|| Error::Other("Dictation doesn't support this Mac's language.".into()))?;
    if !unsafe { recognizer.isAvailable() } {
        return Err(Error::Other(
            "Speech recognition is unavailable right now. If it needs the network, check your \
             connection and try again."
                .into(),
        ));
    }

    let request = unsafe { SFSpeechAudioBufferRecognitionRequest::new() };
    unsafe {
        request.setShouldReportPartialResults(true);
        // Keep the audio on this machine whenever the recognizer can manage
        // it, and fall back to Apple's servers (with their ~1 minute cap)
        // only when it can't.
        request.setRequiresOnDeviceRecognition(recognizer.supportsOnDeviceRecognition());
    }

    let engine = unsafe { AVAudioEngine::new() };
    let input = unsafe { engine.inputNode() };
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

    let generation = NEXT_GENERATION.fetch_add(1, Ordering::SeqCst);

    let tap_request = request.clone();
    let tap = RcBlock::new(
        move |buffer: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
            // Real-time audio thread. Appending is the only thing that may
            // happen here — no session state, no allocation, no emit.
            unsafe { tap_request.appendAudioPCMBuffer(buffer.as_ref()) };
        },
    );
    unsafe {
        input.installTapOnBus_bufferSize_format_block(
            BUS,
            TAP_BUFFER_FRAMES,
            Some(&format),
            RcBlock::as_ptr(&tap),
        );
    }

    let results = result_handler(app.clone(), generation);
    let task = unsafe { recognizer.recognitionTaskWithRequest_resultHandler(&request, &results) };

    unsafe {
        engine.prepare();
        if let Err(e) = engine.startAndReturnError() {
            // Unwind what we just built rather than leaving a tap installed
            // and a task running behind a failed start.
            input.removeTapOnBus(BUS);
            task.cancel();
            return Err(Error::Other(format!(
                "Couldn't start the microphone: {}",
                e.localizedDescription()
            )));
        }
    }

    SESSION.set(Some(Session {
        generation,
        engine,
        input,
        request,
        task,
        _tap: tap,
        stopping: false,
    }));
    // Audio is flowing. Only now can the frontend show a live mic.
    emit_state(&app, State::Listening, None);
    Ok(())
}

/// The recognizer's result handler. Runs on the recognizer's queue — the main
/// queue by default, i.e. the thread `SESSION` lives on.
fn result_handler(
    app: AppHandle,
    generation: u64,
) -> RcBlock<dyn Fn(*mut SFSpeechRecognitionResult, *mut NSError)> {
    RcBlock::new(
        move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
            // Apple passes one or the other, and both may be null.
            if let Some(result) = unsafe { result.as_ref() } {
                let text = unsafe { result.bestTranscription().formattedString() }.to_string();
                let is_final = unsafe { result.isFinal() };
                // A result for an already-replaced session must stay silent,
                // or it would overwrite the new session's transcript.
                if !is_live(generation) {
                    return;
                }
                emit_transcript(&app, text, is_final);
                if is_final {
                    if let Some(session) = claim(generation) {
                        teardown(session);
                        emit_state(&app, State::Stopped, None);
                    }
                }
                return;
            }
            if let Some(error) = unsafe { error.as_ref() } {
                let Some(session) = claim(generation) else {
                    return;
                };
                let message = error.localizedDescription().to_string();
                let stopping = session.stopping;
                teardown(session);
                if stopping {
                    // The expected end of a user-requested stop, not a failure.
                    tracing::debug!(error = %message, "dictation stream ended");
                    emit_state(&app, State::Stopped, None);
                } else {
                    tracing::warn!(error = %message, "dictation failed");
                    emit_state(&app, State::Error, Some(message));
                }
            }
        },
    )
}

pub async fn stop(app: AppHandle) -> Result<()> {
    // The session stays in `SESSION` — the result handler still needs it to
    // deliver the final transcript and own the teardown.
    let stopped = on_main(&app, || {
        // Clone the handles out before calling into ObjC: the rule for
        // `SESSION` is that no borrow is ever held across a framework call.
        let live = SESSION.with_borrow_mut(|slot| {
            let session = slot.as_mut()?;
            session.stopping = true;
            Some((
                session.generation,
                session.engine.clone(),
                session.input.clone(),
                session.request.clone(),
            ))
        });
        let Some((generation, engine, input, request)) = live else {
            // Either genuinely idle, or a `start` is still sitting on the
            // permission prompts. Leave the request for `begin` to consume; a
            // stale flag from the idle case is cleared by the next `start`.
            STOP_PENDING.set(true);
            return None;
        };
        unsafe {
            engine.stop();
            input.removeTapOnBus(BUS);
            // Deliberately not `task.cancel()`: ending the audio is what makes
            // the recognizer flush its final result, and that result is what
            // drives the `stopped` emit. Cancelling would discard it.
            request.endAudio();
        }
        Some(generation)
    })
    .await?;

    // Idle: nothing to stop, and no event.
    let Some(generation) = stopped else {
        return Ok(());
    };

    let handle = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(FLUSH_TIMEOUT).await;
        let emit_to = handle.clone();
        let _ = on_main(&handle, move || {
            // Still live means the final result never arrived; `claim` is what
            // keeps this from double-emitting against the result handler.
            if let Some(session) = claim(generation) {
                tracing::warn!(
                    "dictation: no final result before the flush deadline; forcing stop"
                );
                teardown(session);
                emit_state(&emit_to, State::Stopped, None);
            }
        })
        .await;
    });
    Ok(())
}
