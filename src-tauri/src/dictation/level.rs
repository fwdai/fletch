//! How loud the microphone is right now, for the composer's listening
//! indicator (the level bars in the primary control).
//!
//! The tap on the render thread folds each buffer's RMS into a [`Meter`] — one
//! atomic store, nothing else — and a task off that thread samples it every
//! [`LEVEL_POLL`] and emits `dictation:level`. Both engines share the meter:
//! the local engine already computes the RMS for its silence gate and hands it
//! over; Apple's path reads the buffer once more on the way to the bridge. The
//! meter is a display value only — nothing about the session (its auto-stop,
//! its transcript) depends on it.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use objc2_avf_audio::{AVAudioCommonFormat, AVAudioFormat, AVAudioPCMBuffer};
use tauri::AppHandle;

/// How often a level is emitted while the mic is open. Matches the lerp tick of
/// the bars that display it; anything faster is wasted IPC.
pub(super) const LEVEL_POLL: Duration = Duration::from_millis(90);

/// The dBFS range the bars span. Below the floor is silence (a quiet room on a
/// laptop mic sits around −60 dB); the ceiling is loud, close speech. Linear in
/// dB between the two, because that is how loudness reads.
///
/// The floor is where `whisper::engine::MIN_RMS` sits — the quietest audio the
/// silence detector will ever call speech. Any higher and the bars would render
/// empty for audio the session was still hearing as speech, so the waveform
/// would flatly contradict a mic that refused to stop (see `the_bars_start_
/// where_speech_can`).
const FLOOR_DB: f32 = -54.0;
const CEIL_DB: f32 = -15.0;

/// The latest buffer's loudness, written by the render thread and read by the
/// emitter task.
pub(super) struct Meter {
    /// RMS of the most recent buffer, as `f32` bits.
    rms: AtomicU32,
    /// Whether the tap's buffers can be read as deinterleaved float32 — the
    /// layout every shipped input node delivers. A meter on any other layout
    /// simply reports silence rather than read the wrong memory.
    readable: bool,
    /// Set when the mic is closed, so the emitter stops.
    closed: AtomicBool,
}

impl Meter {
    pub(super) fn new(format: &AVAudioFormat) -> Arc<Self> {
        let readable = unsafe { format.commonFormat() } == AVAudioCommonFormat::PCMFormatFloat32
            && !unsafe { format.isInterleaved() };
        Arc::new(Self {
            rms: AtomicU32::new(0.0f32.to_bits()),
            readable,
            closed: AtomicBool::new(false),
        })
    }

    /// Render thread: note one buffer's RMS.
    pub(super) fn record(&self, rms: f32) {
        self.rms.store(rms.to_bits(), Ordering::Relaxed);
    }

    /// Render thread: measure one buffer and note it. For the path that doesn't
    /// otherwise read the samples (Apple's analyzer takes the buffer whole).
    pub(super) fn record_buffer(&self, buffer: &AVAudioPCMBuffer) {
        if !self.readable {
            return;
        }
        if let Some(rms) = buffer_rms(buffer) {
            self.record(rms);
        }
    }

    /// The current level, 0 (silence) to 1 (loud speech).
    pub(super) fn level(&self) -> f32 {
        normalize(f32::from_bits(self.rms.load(Ordering::Relaxed)))
    }

    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
}

/// RMS of a deinterleaved float32 buffer, channels averaged to mono first.
/// `None` for an empty buffer. Same pass `capture::append_mono` makes, minus the
/// copy.
pub(super) fn buffer_rms(buffer: &AVAudioPCMBuffer) -> Option<f32> {
    let frames = unsafe { buffer.frameLength() } as usize;
    let channels = unsafe { buffer.format().channelCount() } as usize;
    let data = unsafe { buffer.floatChannelData() };
    if data.is_null() || frames == 0 || channels == 0 {
        return None;
    }
    let mut squares = 0.0f64;
    for frame in 0..frames {
        let mut sum = 0.0f32;
        for channel in 0..channels {
            sum += unsafe { *(*data.add(channel)).as_ptr().add(frame) };
        }
        let mono = sum / channels as f32;
        squares += f64::from(mono) * f64::from(mono);
    }
    Some((squares / frames as f64).sqrt() as f32)
}

/// Map a linear RMS onto the bars' 0–1 range: dBFS, linear between
/// [`FLOOR_DB`] and [`CEIL_DB`], clamped.
pub(super) fn normalize(rms: f32) -> f32 {
    if rms <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * rms.log10();
    ((db - FLOOR_DB) / (CEIL_DB - FLOOR_DB)).clamp(0.0, 1.0)
}

/// Emit the level every [`LEVEL_POLL`] until the meter is closed. Emitted on
/// every tick rather than on change, so a consumer that keeps a short history
/// of samples (a scrolling waveform) sees steady time.
pub(super) fn watch(app: AppHandle, session: u64, meter: Arc<Meter>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(LEVEL_POLL).await;
            if meter.is_closed() {
                return;
            }
            super::emit_level(&app, session, meter.level());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_db(db: f32) -> f32 {
        10f32.powf(db / 20.0)
    }

    #[test]
    fn silence_and_the_floor_are_zero() {
        assert_eq!(normalize(0.0), 0.0);
        assert_eq!(normalize(-1.0), 0.0);
        assert_eq!(normalize(from_db(FLOOR_DB)), 0.0);
        assert_eq!(normalize(from_db(FLOOR_DB - 20.0)), 0.0);
    }

    #[test]
    fn the_ceiling_and_above_are_one() {
        assert!((normalize(from_db(CEIL_DB)) - 1.0).abs() < 1e-5);
        assert_eq!(normalize(1.0), 1.0);
    }

    #[test]
    fn linear_in_decibels_between() {
        let mid = (FLOOR_DB + CEIL_DB) / 2.0;
        assert!((normalize(from_db(mid)) - 0.5).abs() < 1e-5);
        let quarter = FLOOR_DB + (CEIL_DB - FLOOR_DB) / 4.0;
        assert!((normalize(from_db(quarter)) - 0.25).abs() < 1e-5);
    }

    /// The bars and the silence detector have to agree about what silence is.
    /// While the floor sat above `MIN_RMS` there was a band where the waveform
    /// read empty and the detector still heard speech — the exact combination
    /// that makes a session which won't end look like a frozen UI instead of a
    /// mic that thinks you're talking.
    #[test]
    fn the_bars_start_where_speech_can() {
        use crate::dictation::whisper::engine::MIN_RMS;

        assert_eq!(normalize(MIN_RMS * 0.99), 0.0);
        assert!(normalize(MIN_RMS * 1.5) > 0.0);
    }

    #[test]
    fn louder_is_never_lower() {
        let levels: Vec<f32> = (0..100).map(|i| normalize(i as f32 / 100.0)).collect();
        assert!(levels.windows(2).all(|w| w[0] <= w[1]));
    }
}
