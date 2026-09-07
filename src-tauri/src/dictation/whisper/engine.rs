//! whisper.cpp inference: a whole clip of 16 kHz mono audio in, one final
//! transcript out.
//!
//! There is no streaming here. whisper.cpp transcribes a complete utterance in
//! one pass — feeding it a growing prefix every few hundred milliseconds costs
//! more than the whole clip and still revises everything — so the session
//! buffers audio while the user speaks and runs the model once on stop. That
//! wait is why the contract has a `transcribing` state at all.
//!
//! The weights are hundreds of megabytes, and loading them takes long enough to
//! be felt between the stop and the text, so a loaded model is cached and
//! reused. It is also given back after [`IDLE_UNLOAD`] of disuse: someone who
//! dictated one sentence this morning shouldn't still be paying half a gigabyte
//! of resident memory for it at lunchtime.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::models::{self, WhisperModel};
use crate::error::{Error, Result};

/// The only input rate whisper.cpp accepts; capture resamples to it.
pub const SAMPLE_RATE: u32 = 16_000;

/// Clips shorter than this can't hold an utterance — they're a tap on the mic
/// button. Whisper answers a too-short clip with an invented sentence rather
/// than nothing ("Thank you.", "[BLANK_AUDIO]"), so they never reach the model.
const MIN_DURATION: Duration = Duration::from_millis(500);

/// RMS below which the clip is a quiet room rather than speech, and whisper
/// would hallucinate the same invented sentences over it. Well under speech at
/// a conversational distance (RMS ~0.02 and up) and well over a muted or
/// unplugged input's noise floor.
///
/// Also `capture`'s silence detector's absolute lower bound, so the two can't
/// disagree about what silence is: audio the gate would refuse to transcribe
/// isn't worth holding a session open for either.
pub const MIN_RMS: f32 = 0.002;

/// How long a loaded model may sit unused before its memory is released.
const IDLE_UNLOAD: Duration = Duration::from_secs(600);

/// The loaded model, shared by every session until it is unloaded.
static CONTEXT: Mutex<Option<Loaded>> = Mutex::new(None);

/// Set while an unload sleeper is pending, so a burst of sessions can't stack
/// up timers.
static UNLOAD_ARMED: AtomicBool = AtomicBool::new(false);

/// One transcription at a time. Inference already saturates the GPU or a
/// core, so two wouldn't finish sooner — and serialising them is what makes a
/// model switch clean: the previous decode has dropped its handle on the old
/// context before [`context`] clears the slot and loads the new one, so the two
/// models are never resident together. Taken before [`CONTEXT`], never after.
static DECODING: Mutex<()> = Mutex::new(());

struct Loaded {
    ctx: Arc<WhisperContext>,
    /// The weights this context came from. The user can change the model in
    /// Settings between one session and the next, and a `WhisperContext` says
    /// nothing about which file it holds — so the cache is keyed on the path
    /// rather than assumed to be the current choice.
    path: PathBuf,
    /// When the model was last handed to a transcription — the unload sleeper
    /// compares this against [`IDLE_UNLOAD`] rather than trusting its own
    /// deadline, so a session that started while it slept keeps the model.
    used: Instant,
}

/// Transcribe one whole clip of 16 kHz mono audio with `model`, the user's
/// choice as the caller read it from settings — the engine has no database
/// handle of its own, and a session should use the model that was chosen when
/// it stopped.
///
/// `Ok("")` for a clip with no speech in it (see [`MIN_DURATION`] and
/// [`MIN_RMS`]) — that answer costs nothing, because the gate runs before the
/// model is even loaded.
pub async fn transcribe(model: &'static WhisperModel, samples: Vec<f32>) -> Result<String> {
    if !has_speech(&samples) {
        return Ok(String::new());
    }
    // Inference pins a core for seconds; it has no business on the async
    // runtime's worker threads.
    tokio::task::spawn_blocking(move || {
        let _one_at_a_time = DECODING.lock();
        let ctx = context(model)?;
        decode(&ctx, &samples)
    })
    .await
    .map_err(|e| Error::Other(format!("dictation: transcription task failed: {e}")))?
}

/// Cheap "is there anything in here to transcribe" gate.
fn has_speech(samples: &[f32]) -> bool {
    let min_samples = (f64::from(SAMPLE_RATE) * MIN_DURATION.as_secs_f64()) as usize;
    samples.len() >= min_samples && rms(samples) >= MIN_RMS
}

fn rms(samples: &[f32]) -> f32 {
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// The cached context for `model`, loading it on first use. Also stamps the
/// load as used, which is what keeps [`arm_unload`]'s sleeper from taking it
/// out from under a session that has only just started.
fn context(model: &WhisperModel) -> Result<Arc<WhisperContext>> {
    let path = models::installed_path(model).ok_or_else(|| {
        Error::Other(
            "The local dictation model isn't installed. Download it in Settings > Dictation."
                .into(),
        )
    })?;
    let mut slot = CONTEXT.lock();
    if let Some(loaded) = slot.as_mut().filter(|l| l.path == path) {
        loaded.used = Instant::now();
        return Ok(loaded.ctx.clone());
    }
    // Drop a context for the previous choice before loading the new one, so a
    // switch doesn't hold both models resident. This is the last handle: any
    // decode that held another finished before we took `DECODING`.
    *slot = None;
    let ctx = Arc::new(load(&path)?);
    *slot = Some(Loaded {
        ctx: ctx.clone(),
        path,
        used: Instant::now(),
    });
    // Armed under the same lock the sleeper unloads under, so "a model is
    // loaded" and "a sleeper is watching it" change together: the sleeper can't
    // retire between our check and our load and leave this context resident
    // forever.
    arm_unload();
    Ok(ctx)
}

fn load(path: &Path) -> Result<WhisperContext> {
    // whisper.cpp and GGML print a page of model and backend detail per load
    // straight to stderr. Route it through `tracing` like everything else the
    // app logs; the call is idempotent.
    whisper_rs::install_logging_hooks();
    let started = Instant::now();
    let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default()).map_err(
        |e| {
            Error::Other(format!(
                "Couldn't load the local dictation model: {e}. Re-download it in Settings."
            ))
        },
    )?;
    tracing::info!(
        model = %path.display(),
        elapsed_ms = started.elapsed().as_millis(),
        "dictation: loaded whisper model"
    );
    Ok(ctx)
}

/// Release the model once it has gone [`IDLE_UNLOAD`] without a transcription.
/// One sleeper at a time; it re-sleeps while the model is still in use rather
/// than unloading on its own deadline.
///
/// Called with [`CONTEXT`] held (see [`context`]), and the sleeper disarms
/// itself while holding it too, so the flag and the cache never disagree.
/// Spawned on the app runtime by handle rather than `tokio::spawn`, because
/// the caller is on a blocking thread.
fn arm_unload() {
    if UNLOAD_ARMED.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(IDLE_UNLOAD).await;
            let mut slot = CONTEXT.lock();
            let idle = !slot
                .as_ref()
                .is_some_and(|l| l.used.elapsed() < IDLE_UNLOAD);
            if idle {
                *slot = None;
                UNLOAD_ARMED.store(false, Ordering::SeqCst);
                return;
            }
        }
    });
}

/// Run the model. Split from [`transcribe`] so the end-to-end test can point it
/// at a model of its own instead of the installed one.
fn decode(ctx: &WhisperContext, samples: &[f32]) -> Result<String> {
    let mut state = ctx.create_state().map_err(transcription_failed)?;

    // Greedy: dictation is one short utterance going straight into a text box,
    // where a beam search buys accuracy the user would rather have as latency.
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    // whisper.cpp's own detect-from-the-audio sentinel — better than guessing
    // from the system locale, which says nothing about what the user speaks.
    params.set_language(Some("auto"));
    params.set_translate(false);
    // One utterance, one text box: segment framing and timestamps would only
    // have to be stripped back out.
    params.set_no_timestamps(true);
    params.set_token_timestamps(false);
    // Leading and trailing silence otherwise decodes as invented words. This is
    // whisper.cpp's own guard against it, and the counterpart to `has_speech`.
    params.set_suppress_blank(true);
    // Nothing here has a console to print to.
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);

    state.full(params, samples).map_err(transcription_failed)?;

    let mut text = String::new();
    for segment in state.as_iter() {
        text.push_str(&segment.to_str_lossy().map_err(transcription_failed)?);
    }
    Ok(text.trim().to_string())
}

fn transcription_failed(e: whisper_rs::WhisperError) -> Error {
    Error::Other(format!("Dictation couldn't transcribe the recording: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate that keeps whisper from inventing a sentence out of a quiet
    /// room. It runs before the model is loaded, which is the point — this test
    /// needs no weights. Point `FLETCH_WHISPER_TEST_SILENT_WAV` at a real
    /// recording to try one instead of synthetic zeroes.
    #[tokio::test]
    async fn silence_never_reaches_the_model() {
        let silence = match std::env::var_os("FLETCH_WHISPER_TEST_SILENT_WAV") {
            Some(p) => read_wav(Path::new(&p)),
            None => vec![0.0; SAMPLE_RATE as usize * 2],
        };
        // Any catalog entry will do: the gate answers before the model matters.
        let model = models::platform_default();
        assert_eq!(transcribe(model, silence).await.unwrap(), "");
        // Too short to be an utterance, however loud.
        assert_eq!(
            transcribe(model, vec![0.5; SAMPLE_RATE as usize / 10])
                .await
                .unwrap(),
            ""
        );
    }

    /// The real thing, against real weights — ignored because they are hundreds
    /// of megabytes CI has no reason to download:
    ///
    /// ```sh
    /// say -o /tmp/hello.wav --data-format=LEI16@16000 "hello world, this is a dictation test"
    /// FLETCH_WHISPER_TEST_MODEL=/tmp/ggml-small.en-q8_0.bin \
    /// FLETCH_WHISPER_TEST_WAV=/tmp/hello.wav \
    ///   cargo test --manifest-path src-tauri/Cargo.toml transcribes -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs FLETCH_WHISPER_TEST_MODEL and FLETCH_WHISPER_TEST_WAV"]
    fn transcribes_a_wav() {
        let model = std::env::var("FLETCH_WHISPER_TEST_MODEL").expect("FLETCH_WHISPER_TEST_MODEL");
        let wav = std::env::var("FLETCH_WHISPER_TEST_WAV").expect("FLETCH_WHISPER_TEST_WAV");
        let samples = read_wav(Path::new(&wav));
        assert!(has_speech(&samples), "{wav} has no speech in it");

        let ctx = load(Path::new(&model)).unwrap();
        let text = decode(&ctx, &samples).unwrap();
        println!("transcript: {text:?}");
        assert!(
            text.to_lowercase().contains("hello world"),
            "expected the phrase in {text:?}"
        );
    }

    /// Minimal reader for the one shape this test takes: 16 kHz mono 16-bit
    /// PCM, as `say --data-format=LEI16@16000` writes it.
    fn read_wav(path: &Path) -> Vec<f32> {
        let bytes = std::fs::read(path).expect("read wav");
        assert_eq!(&bytes[0..4], b"RIFF", "{} is not a WAV", path.display());
        // Walk the chunk list rather than assuming a fixed header: `say` writes
        // a plain fmt+data pair, but other writers add LIST/fact chunks.
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
            if id == b"fmt " {
                let channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                let bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
                assert_eq!((channels, rate, bits), (1, SAMPLE_RATE, 16), "{path:?}");
            }
            if id == b"data" {
                return body
                    .chunks_exact(2)
                    .map(|s| f32::from(i16::from_le_bytes([s[0], s[1]])) / 32768.0)
                    .collect();
            }
            at += 8 + size + (size & 1);
        }
        panic!("{} has no data chunk", path.display());
    }
}
