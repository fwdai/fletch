//! The Rust side of the `SpeechAnalyzer` bridge. The Swift half is
//! `swift/SpeechBridge.swift`, compiled by `build.rs` into a static library;
//! this file declares its C surface and wraps it so `apple` never sees a raw
//! pointer. Keep the two in sync.
//!
//! # Ownership
//!
//! A session's callback context is a boxed [`Events`] handed to Swift at start
//! and freed when Swift reports the session object gone (`on_release`, fired
//! from its `deinit`). That is after the last callback by construction — the
//! tasks that call back hold the object alive — so no callback can observe a
//! freed context. The one-shot replies (`prepare`, `supported`) box a channel
//! sender the same way and free it in the single reply.
//!
//! Callbacks arrive on Swift's executors, never on the main thread. Marshalling
//! onto it is the caller's job (see `apple::spawn_main`).

use std::ffi::{c_char, c_void, CStr};
use std::ptr::NonNull;

use objc2_avf_audio::{AVAudioFormat, AVAudioPCMBuffer};
use tokio::sync::oneshot;

use crate::error::{Error, Result};

type ReadyCallback = unsafe extern "C" fn(*mut c_void, bool, *const c_char);
type ResultCallback = unsafe extern "C" fn(*mut c_void, *const c_char, bool);
type ErrorCallback = unsafe extern "C" fn(*mut c_void, *const c_char);
type ReleaseCallback = unsafe extern "C" fn(*mut c_void);

extern "C" {
    fn fletch_speech_availability(ctx: *mut c_void, done: ReadyCallback);
    fn fletch_speech_prepare(ctx: *mut c_void, done: ReadyCallback);
    fn fletch_speech_start(
        format: *const AVAudioFormat,
        ctx: *mut c_void,
        on_result: ResultCallback,
        on_error: ErrorCallback,
        on_release: ReleaseCallback,
    ) -> *mut c_void;
    fn fletch_speech_feed(session: *mut c_void, buffer: *const AVAudioPCMBuffer);
    fn fletch_speech_finish(session: *mut c_void);
    fn fletch_speech_cancel(session: *mut c_void);
}

/// A string from the bridge, or `None` for a null pointer.
///
/// # Safety
/// `ptr` is null or a NUL-terminated string valid for the call.
unsafe fn message(ptr: *const c_char) -> Option<String> {
    (!ptr.is_null()).then(|| CStr::from_ptr(ptr).to_string_lossy().into_owned())
}

type Reply = (bool, Option<String>);

/// Ask the bridge a question whose answer arrives on a callback, exactly once.
async fn ask(question: unsafe extern "C" fn(*mut c_void, ReadyCallback)) -> Reply {
    let (tx, rx) = oneshot::channel::<Reply>();
    let ctx = Box::into_raw(Box::new(tx));
    unsafe { question(ctx.cast(), on_ready) };
    rx.await
        .unwrap_or((false, Some("Speech recognition didn't answer.".into())))
}

unsafe extern "C" fn on_ready(ctx: *mut c_void, ok: bool, why: *const c_char) {
    // The bridge replies exactly once per request, so this is the box's only
    // owner and the reply is where it dies.
    let tx: Box<oneshot::Sender<Reply>> = Box::from_raw(ctx.cast());
    let _ = tx.send((ok, message(why)));
}

/// Can the default engine run on this Mac: macOS 26+ with a model for its
/// language. Never prompts and never downloads.
pub async fn supported() -> bool {
    ask(fletch_speech_availability).await.0
}

/// Settle everything a session needs that is asynchronous — locale, model
/// assets, audio format — so [`Session::start`] can be synchronous. A missing
/// model starts its download and is reported as an error with a readable
/// message; the next attempt succeeds once it has landed.
pub async fn prepare() -> Result<()> {
    match ask(fletch_speech_prepare).await {
        (true, _) => Ok(()),
        (false, why) => Err(Error::Other(why.unwrap_or_else(|| {
            "Speech recognition isn't available on this Mac.".into()
        }))),
    }
}

/// What a session reports back. Both arrive on arbitrary threads, `on_result`
/// with the whole running transcript (`is_final` once, last), `on_error` at
/// most once and then nothing more.
pub struct Events {
    pub on_result: Box<dyn Fn(String, bool) + Send + Sync>,
    pub on_error: Box<dyn Fn(String) + Send + Sync>,
}

unsafe extern "C" fn on_result(ctx: *mut c_void, text: *const c_char, is_final: bool) {
    let events = &*(ctx as *const Events);
    (events.on_result)(message(text).unwrap_or_default(), is_final);
}

unsafe extern "C" fn on_error(ctx: *mut c_void, why: *const c_char) {
    let events = &*(ctx as *const Events);
    (events.on_error)(message(why).unwrap_or_else(|| "Speech recognition failed.".into()));
}

unsafe extern "C" fn on_release(ctx: *mut c_void) {
    drop(Box::from_raw(ctx as *mut Events));
}

/// A live analyzer session: the bridge's retained handle. Copies are the same
/// session, so `stop` can lift it out of the session slot the way it does a
/// `Retained` handle.
#[derive(Clone, Copy)]
pub struct Session(NonNull<c_void>);

impl Session {
    /// Start a session for microphone buffers in `format`. Fails when
    /// [`prepare`] hasn't succeeded yet.
    pub fn start(format: &AVAudioFormat, events: Events) -> Result<Self> {
        let ctx = Box::into_raw(Box::new(events));
        let handle =
            unsafe { fletch_speech_start(format, ctx.cast(), on_result, on_error, on_release) };
        match NonNull::new(handle) {
            Some(handle) => Ok(Session(handle)),
            None => {
                // No session was made, so no `on_release` will come: the box
                // is ours again.
                drop(unsafe { Box::from_raw(ctx) });
                Err(Error::Other(
                    "Speech recognition isn't ready yet. Try again.".into(),
                ))
            }
        }
    }

    /// Real-time render thread: hand a tap buffer over and return.
    pub fn feed(&self, buffer: &AVAudioPCMBuffer) {
        unsafe { fletch_speech_feed(self.0.as_ptr(), buffer) }
    }

    /// No more audio. The final result follows on `on_result`.
    pub fn finish(&self) {
        unsafe { fletch_speech_finish(self.0.as_ptr()) }
    }

    /// Drop the session. Must be this handle's (and its copies') last use.
    pub fn cancel(&self) {
        unsafe { fletch_speech_cancel(self.0.as_ptr()) }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;
    use crate::dictation::capture::{channel, pcm_buffer, standard_mono};

    /// Set to a **16 kHz mono 16-bit** WAV saying "hello world" to have the
    /// bridge transcribe real speech; otherwise a second of silence goes
    /// through, which still exercises every entry point and the final result.
    const WAV_ENV: &str = "FLETCH_SPEECH_TEST_WAV";
    const RATE: f64 = 16_000.0;

    /// The data chunk of a 16-bit PCM WAV, as float samples.
    fn wav_samples(bytes: &[u8]) -> Vec<f32> {
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            let body = &bytes[at + 8..(at + 8 + len).min(bytes.len())];
            if id == b"data" {
                return body
                    .chunks_exact(2)
                    .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / i16::MAX as f32)
                    .collect();
            }
            at += 8 + len + (len & 1);
        }
        panic!("no data chunk in WAV");
    }

    /// The whole bridge, without a microphone: prepare, start, feed, finish,
    /// then the final result and the release of the callback context, in that
    /// order. Skips (rather than fails) where the engine can't run — an older
    /// macOS, an unsupported language, or a model still downloading — since
    /// none of those is a defect in this code.
    #[tokio::test]
    async fn transcribes_buffers_end_to_end() {
        if !supported().await {
            eprintln!("skipping: SpeechAnalyzer isn't available on this Mac");
            return;
        }
        if let Err(e) = prepare().await {
            eprintln!("skipping: {e}");
            return;
        }

        let samples = match std::env::var(WAV_ENV) {
            Ok(path) => wav_samples(&std::fs::read(path).unwrap()),
            Err(_) => vec![0.0; RATE as usize],
        };
        let expect_speech = std::env::var(WAV_ENV).is_ok();

        let (tx, rx) = mpsc::channel::<(String, bool)>();
        let (released_tx, released_rx) = mpsc::channel::<()>();
        struct Released(mpsc::Sender<()>);
        impl Drop for Released {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        let released = Released(released_tx);
        let errors = tx.clone();
        let session = Session::start(
            &standard_mono(RATE).unwrap(),
            Events {
                on_result: Box::new(move |text, is_final| {
                    let _ = tx.send((text, is_final));
                }),
                on_error: Box::new(move |why| {
                    // Dropping `released` when Swift frees the context is the
                    // release signal, whichever closure it travels in.
                    let _ = &released;
                    let _ = errors.send((format!("ERROR: {why}"), true));
                }),
            },
        )
        .unwrap();

        // Tap-sized chunks, the way the mic delivers them.
        for chunk in samples.chunks(1024) {
            let buffer = pcm_buffer(&standard_mono(RATE).unwrap(), chunk.len() as u32).unwrap();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    chunk.as_ptr(),
                    channel(&buffer).unwrap().as_ptr(),
                    chunk.len(),
                );
                buffer.setFrameLength(chunk.len() as u32);
            }
            session.feed(&buffer);
        }
        session.finish();

        let deadline = Duration::from_secs(30);
        let text = loop {
            let (text, is_final) = rx.recv_timeout(deadline).expect("a final result");
            assert!(!text.starts_with("ERROR: "), "{text}");
            if is_final {
                break text;
            }
        };
        if expect_speech {
            // Punctuation and casing are the analyzer's call ("Hello, world.");
            // the words are what matter.
            let words: String = text
                .to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric() || c.is_whitespace())
                .collect();
            assert!(words.contains("hello world"), "transcript was {text:?}");
        }

        session.cancel();
        released_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the callback context to be released after cancel");
    }
}
