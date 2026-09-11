//! Dictation for a paired phone: the phone captures its microphone, streams
//! PCM over the remote protocol, and this Mac runs the local whisper.cpp engine
//! on it — the same `engine::transcribe` the desktop composer's stop calls.
//!
//! The wire shape is four ops (`docs/remote-protocol.md`, "Dictation"):
//! `dictation_begin` opens a session, `dictation_audio` appends a chunk while
//! the user talks, `dictation_end` transcribes and answers with the text, and
//! `dictation_cancel` throws the audio away. `dictation_status` beside them
//! says whether any of that would work, so the phone can hide its mic button
//! the way the desktop composer hides its own, and carries the "Stop after a
//! pause" setting: the pause lives in frames that never cross the wire, so the
//! phone's own capture is the only thing that can honour a setting this Mac
//! owns.
//!
//! No events, no partials: whisper.cpp has nothing to say until the whole clip
//! is in, so the transcript is simply the reply to `dictation_end`, and the
//! phone shows "transcribing" for as long as that request is outstanding.
//!
//! Audio arrives as 16-bit little-endian mono PCM at the phone's own rate and
//! is resampled here with the same converter the desktop path uses, so the
//! phone never has to know what the model wants. Everything a session holds is
//! bounded: a capture cap in seconds (the desktop's), a cap on concurrent
//! sessions, and an idle sweep for a phone that vanished mid-sentence. Nothing
//! is written to disk.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use base64::Engine as _;
use parking_lot::Mutex;
use serde::Serialize;
use tauri::AppHandle;

use crate::error::{Error, Result};

/// Sessions that may be open at once, across every paired phone. A dictation is
/// one utterance from one person; this is a bound on abuse, not on use.
pub const MAX_SESSIONS: usize = 4;

/// How long a session may go without a chunk before it is swept. A phone that
/// loses its connection mid-sentence never sends `dictation_end`, and the
/// buffer it left behind is minutes of audio.
pub const SESSION_IDLE: Duration = Duration::from_secs(60);

/// Largest decoded chunk accepted. A second of 48 kHz mono is under 100 KB; a
/// phone that sends this much in one frame is not streaming.
pub const MAX_CHUNK_BYTES: usize = 2 * 1024 * 1024;

/// Sample rates a phone could plausibly report. Outside this the numbers are a
/// bug, and resampling from them would only produce garbage for the model.
const RATE_RANGE: std::ops::RangeInclusive<f64> = 8_000.0..=192_000.0;

#[derive(Clone, Serialize)]
pub struct Status {
    /// `dictation_begin` would succeed right now.
    pub available: bool,
    /// Why not, in words the phone shows as-is. `None` when available.
    pub reason: Option<String>,
    /// Settings › Dictation's "Stop after a pause", as this Mac has it. The
    /// phone arms its silence monitor only when this is true; with it off
    /// nothing but a tap ends the session, which is what the desktop does
    /// (`capture::should_auto_stop` refuses both the pause and the never-spoke
    /// deadline). What the host bounds by itself is the *memory*, through the
    /// capture cap — not the session: `SESSION_IDLE` is refreshed by every
    /// chunk, and a phone with the setting off keeps sending one a second, so
    /// an abandoned session holds its mic until the user taps, leaves, or the
    /// link drops. That is the desktop's behaviour too.
    pub auto_stop: bool,
}

impl Status {
    /// Split from [`status`] so the tests can build one without an
    /// `AppHandle`. `auto_stop` comes from the in-memory mirror, not the DB:
    /// `set_dictation_auto_stop` keeps it current and boot restores it. It is
    /// answered whether or not this Mac is ready to transcribe — the two say
    /// different things, and a phone that asks while the model is still
    /// downloading should still learn how the session will end once it isn't.
    fn new(readiness: Result<()>) -> Self {
        Self {
            available: readiness.is_ok(),
            reason: readiness.err().map(|e| e.to_string()),
            auto_stop: super::auto_stop(),
        }
    }
}

#[derive(Serialize)]
pub struct Begun {
    pub session: String,
}

#[derive(Serialize)]
pub struct Ended {
    /// The transcript, or empty for a clip with no speech in it (see the
    /// engine's silence gate).
    pub text: String,
}

struct Session {
    /// Fixed by the first chunk; every later chunk must agree.
    rate: Option<f64>,
    samples: Vec<i16>,
    touched: Instant,
}

/// The open sessions. Separate from the global so the tests can drive one of
/// their own, with a clock they control.
pub(super) struct Store {
    sessions: Mutex<HashMap<String, Session>>,
}

/// What `end` hands to the transcriber: the audio and the rate it was captured at.
// Only the macOS transcriber reads it; the stub elsewhere refuses before looking.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) struct Clip {
    pub rate: f64,
    pub samples: Vec<i16>,
}

/// The process-wide store the ops act on. `OnceLock` rather than `LazyLock`:
/// the crate's MSRV predates the latter.
static STORE: OnceLock<Store> = OnceLock::new();

fn store() -> &'static Store {
    STORE.get_or_init(Store::new)
}

impl Store {
    pub(super) fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Open a session. Sweeps idle ones first, so a phone that dropped out
    /// never blocks the next one from starting.
    pub(super) fn begin(&self, now: Instant) -> Result<String> {
        let mut sessions = self.sessions.lock();
        sessions.retain(|_, s| now.duration_since(s.touched) < SESSION_IDLE);
        if sessions.len() >= MAX_SESSIONS {
            return Err(Error::Other(
                "Too many dictation sessions are open on your Mac. Try again in a minute.".into(),
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        sessions.insert(
            id.clone(),
            Session {
                rate: None,
                samples: Vec::new(),
                touched: now,
            },
        );
        Ok(id)
    }

    /// Append one chunk of 16-bit little-endian mono PCM. Audio past the
    /// capture cap is dropped rather than refused, exactly as the desktop's mic
    /// tap does: the transcript is truncated instead of the session failing.
    pub(super) fn append(&self, id: &str, rate: f64, pcm: &[u8], now: Instant) -> Result<()> {
        if !RATE_RANGE.contains(&rate) {
            return Err(Error::Other(format!(
                "dictation: {rate} Hz is not a usable sample rate"
            )));
        }
        if pcm.len() % 2 != 0 {
            return Err(Error::Other(
                "dictation: audio chunk is not whole 16-bit samples".into(),
            ));
        }
        let mut sessions = self.sessions.lock();
        let session = sessions.get_mut(id).ok_or_else(unknown_session)?;
        match session.rate {
            None => session.rate = Some(rate),
            Some(fixed) if fixed != rate => {
                return Err(Error::Other(format!(
                    "dictation: sample rate changed mid-session ({fixed} Hz to {rate} Hz)"
                )));
            }
            Some(_) => {}
        }
        session.touched = now;
        let limit = (rate * super::MAX_CAPTURE_SECS) as usize;
        let room = limit.saturating_sub(session.samples.len());
        let samples = pcm
            .chunks_exact(2)
            .take(room)
            .map(|b| i16::from_le_bytes([b[0], b[1]]));
        session.samples.extend(samples);
        Ok(())
    }

    /// Close the session and take its audio. `None` rate means no chunk ever
    /// arrived, which the caller treats as an empty clip.
    pub(super) fn take(&self, id: &str) -> Result<Clip> {
        let session = self
            .sessions
            .lock()
            .remove(id)
            .ok_or_else(unknown_session)?;
        Ok(Clip {
            rate: session.rate.unwrap_or(16_000.0),
            samples: session.samples,
        })
    }

    /// Drop a session's audio. Idempotent: a cancel for a session that was
    /// already swept or ended has nothing left to do.
    pub(super) fn cancel(&self, id: &str) {
        self.sessions.lock().remove(id);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.sessions.lock().len()
    }
}

fn unknown_session() -> Error {
    Error::Other("dictation: unknown session — it may have timed out; start again".into())
}

fn decode(pcm_base64: &str) -> Result<Vec<u8>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(pcm_base64)
        .map_err(|e| Error::Other(format!("dictation: audio chunk is not base64: {e}")))?;
    if bytes.len() > MAX_CHUNK_BYTES {
        return Err(Error::Other(format!(
            "dictation: audio chunk of {} bytes is over the {} byte limit",
            bytes.len(),
            MAX_CHUNK_BYTES
        )));
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// The ops, as the dispatcher calls them.

/// Whether a phone can dictate through this Mac right now, if not what the user
/// has to do about it, and how a session is meant to end. Cheap — two settings
/// reads, an atomic load and a metadata stat.
pub fn status(app: &AppHandle) -> Status {
    Status::new(readiness(app))
}

pub fn begin(app: &AppHandle) -> Result<Begun> {
    // Checked up front so the phone learns before it opens its mic, not after
    // the user has spoken a paragraph.
    readiness(app)?;
    Ok(Begun {
        session: store().begin(Instant::now())?,
    })
}

pub fn append(session: &str, rate: f64, pcm_base64: &str) -> Result<()> {
    let pcm = decode(pcm_base64)?;
    store().append(session, rate, &pcm, Instant::now())
}

pub fn cancel(session: &str) {
    store().cancel(session);
}

/// Close the session and transcribe what it holds. The buffer is consumed
/// whatever happens next, so a failed transcription doesn't leave audio behind.
pub async fn end(app: &AppHandle, session: &str) -> Result<Ended> {
    let clip = store().take(session)?;
    transcribe(app, clip).await.map(|text| Ended { text })
}

/// The local engine has to be chosen *and* have its weights on disk — the
/// same two conditions `dictation::engine` requires before a desktop session
/// uses it. There is no fallback to the platform recognizer here: it needs a
/// microphone this Mac doesn't have.
#[cfg(target_os = "macos")]
fn readiness(app: &AppHandle) -> Result<()> {
    use tauri::Manager;
    let (enabled, model) = super::engine_settings(&app.state::<crate::DbState>());
    if !enabled {
        return Err(Error::Other(
            "Local dictation is off on your Mac. Turn on the Whisper engine in Settings › Dictation."
                .into(),
        ));
    }
    if super::whisper::models::installed_path(model).is_none() {
        return Err(Error::Other(
            "The dictation model isn't downloaded on your Mac yet. Finish the download in \
             Settings › Dictation."
                .into(),
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
async fn transcribe(app: &AppHandle, clip: Clip) -> Result<String> {
    use tauri::Manager;
    // Read at the end, like the desktop's stop: the model a session transcribes
    // with is the one Settings showed when it ended.
    let (_, model) = super::engine_settings(&app.state::<crate::DbState>());
    let Clip { rate, samples } = clip;
    let samples = tokio::task::spawn_blocking(move || {
        let samples: Vec<f32> = samples.iter().map(|s| f32::from(*s) / 32768.0).collect();
        super::capture::resample(rate, samples)
    })
    .await
    .map_err(|e| Error::Other(format!("dictation: resampling task failed: {e}")))??;
    super::whisper::engine::transcribe(model, samples).await
}

#[cfg(not(target_os = "macos"))]
fn readiness(_app: &AppHandle) -> Result<()> {
    Err(Error::Other(
        "Dictation from a phone needs a Mac host: whisper.cpp is only built there.".into(),
    ))
}

#[cfg(not(target_os = "macos"))]
async fn transcribe(_app: &AppHandle, _clip: Clip) -> Result<String> {
    Err(Error::Other(
        "Dictation from a phone needs a Mac host: whisper.cpp is only built there.".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn chunks_accumulate_in_order_and_end_takes_them() {
        let store = Store::new();
        let now = Instant::now();
        let id = store.begin(now).unwrap();
        store.append(&id, 48_000.0, &pcm(&[1, 2, 3]), now).unwrap();
        store.append(&id, 48_000.0, &pcm(&[4]), now).unwrap();

        let clip = store.take(&id).unwrap();
        assert_eq!(clip.rate, 48_000.0);
        assert_eq!(clip.samples, [1, 2, 3, 4]);
        assert!(store.take(&id).is_err(), "a session ends once");
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn a_session_that_never_got_audio_ends_as_an_empty_clip() {
        let store = Store::new();
        let id = store.begin(Instant::now()).unwrap();
        let clip = store.take(&id).unwrap();
        assert!(clip.samples.is_empty());
    }

    #[test]
    fn bad_chunks_are_refused() {
        let store = Store::new();
        let now = Instant::now();
        let id = store.begin(now).unwrap();
        assert!(store.append("nope", 48_000.0, &pcm(&[1]), now).is_err());
        assert!(
            store.append(&id, 48_000.0, &[1, 2, 3], now).is_err(),
            "odd byte count"
        );
        assert!(
            store.append(&id, 1.0, &pcm(&[1]), now).is_err(),
            "absurd rate"
        );
        store.append(&id, 48_000.0, &pcm(&[1]), now).unwrap();
        assert!(
            store.append(&id, 44_100.0, &pcm(&[1]), now).is_err(),
            "the rate is fixed by the first chunk"
        );
        // None of the refusals touched the buffer.
        assert_eq!(store.take(&id).unwrap().samples, [1]);
    }

    #[test]
    fn audio_past_the_capture_cap_is_dropped_not_refused() {
        let store = Store::new();
        let now = Instant::now();
        let id = store.begin(now).unwrap();
        let rate = 8_000.0;
        let limit = (rate * crate::dictation::MAX_CAPTURE_SECS) as usize;
        let big = vec![7i16; limit + 100];
        store.append(&id, rate, &pcm(&big), now).unwrap();
        store.append(&id, rate, &pcm(&[9]), now).unwrap();
        assert_eq!(store.take(&id).unwrap().samples.len(), limit);
    }

    #[test]
    fn idle_sessions_are_swept_and_live_ones_capped() {
        let store = Store::new();
        let t0 = Instant::now();
        let stale = store.begin(t0).unwrap();
        for _ in 1..MAX_SESSIONS {
            store.begin(t0).unwrap();
        }
        assert!(store.begin(t0).is_err(), "the cap holds while all are live");

        let later = t0 + SESSION_IDLE;
        let fresh = store.begin(later).unwrap();
        assert!(
            store.append(&stale, 16_000.0, &pcm(&[1]), later).is_err(),
            "the idle session was swept to make room"
        );
        store.append(&fresh, 16_000.0, &pcm(&[1]), later).unwrap();
    }

    #[test]
    fn cancel_is_idempotent() {
        let store = Store::new();
        let id = store.begin(Instant::now()).unwrap();
        store.cancel(&id);
        store.cancel(&id);
        assert!(store.take(&id).is_err());
    }

    /// The phone cannot read the Mac's settings any other way, so a stale or
    /// hardcoded value here is the opt-out silently not working.
    #[test]
    fn status_reports_the_auto_stop_setting_as_it_stands() {
        super::super::set_auto_stop(false);
        assert!(!Status::new(Ok(())).auto_stop);
        // Not ready to transcribe is a separate fact: the setting is still the
        // honest answer, and the phone that reconnects later depends on it.
        assert!(!Status::new(Err(Error::Other("model missing".into()))).auto_stop);

        super::super::set_auto_stop(true);
        let status = Status::new(Ok(()));
        assert!(status.auto_stop);
        assert!(status.available && status.reason.is_none());
    }

    #[test]
    fn chunks_are_base64_and_bounded() {
        assert_eq!(decode("AAEA/w==").unwrap(), [0, 1, 0, 255]);
        assert!(decode("not base64!").is_err());
        let over = base64::engine::general_purpose::STANDARD.encode(vec![0u8; MAX_CHUNK_BYTES + 2]);
        assert!(decode(&over).is_err());
    }
}
