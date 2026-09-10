//! The local engine's half of the mic tap: where the audio goes when there is
//! no recognizer streaming results back.
//!
//! whisper.cpp wants one complete clip at 16 kHz mono, so a session accumulates
//! [`Pcm`] while the user speaks and [`transcribe`] runs the model once on stop.
//! The two conversions are deliberately split by thread:
//!
//! - **Channel averaging happens in the tap**, on Apple's real-time render
//!   thread, because it is a handful of adds per frame into a pre-sized buffer.
//! - **Resampling happens at stop**, off both the render thread and the main
//!   thread, because it is a framework call (`AVAudioConverter`) with an
//!   allocation behind it. Decimating 48 kHz by taking every third sample would
//!   be cheap enough for the tap, but folds everything above 8 kHz back into the
//!   speech band, and the model hears the aliases.

use std::cell::Cell;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::AllocAnyThread;
use objc2_avf_audio::{
    AVAudioBuffer, AVAudioCommonFormat, AVAudioConverter, AVAudioConverterInputStatus,
    AVAudioConverterOutputStatus, AVAudioFormat, AVAudioPCMBuffer, AVAudioTime,
};
use parking_lot::Mutex;

use super::apple::Tap;
use super::level::Meter;
use super::whisper::engine;
use crate::error::{Error, Result};

use super::MAX_CAPTURE_SECS;

/// How long a pause has to last, once something has been said, for the session
/// to end itself — long enough to think mid-sentence, short enough that the
/// text lands while the user is still looking at the composer.
///
/// This and the two constants below are the whole of the hands-free policy. The
/// Settings opt-out (`dictation_auto_stop`) gates the monitor that reads them
/// rather than changing them — see [`should_auto_stop`].
const SILENCE_STOP: Duration = Duration::from_secs(2);

/// How long a session in which nothing was ever said stays open: the user
/// clicked the mic and walked away. The clip has no speech in it, so the
/// engine's gate answers empty and the session just ends.
const NO_SPEECH_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the monitor looks. Well under [`SILENCE_STOP`], and cheap: two
/// relaxed atomic loads.
pub(super) const SILENCE_POLL: Duration = Duration::from_millis(100);

/// How fast the floor may climb back toward the room once a quiet moment has
/// pulled it down, in dB per second.
///
/// The floor drops to a quieter buffer instantly but recovers only at this
/// rate, which is what keeps a transient dropout from pinning it: the mic's
/// first buffers after `startAndReturnError` are commonly near-silent, and
/// before this existed that one moment set the threshold for the whole session
/// (see `a_quiet_dip_does_not_pin_the_floor`).
///
/// The rate is the whole of the tradeoff. Too slow and the room reads as speech
/// for seconds after a dip, holding the session open; too fast and an unbroken
/// stretch of talking lifts the floor into its own range and the session cuts
/// out mid-sentence. Six dB/s forgets a dropout in about a second and would
/// need tens of seconds of gapless speech to reach it — and real speech is not
/// gapless, so the instant drop wins there on every stop consonant.
const FLOOR_RISE_DB_PER_SEC: f32 = 6.0;

/// Longest gap the floor is aged across. Buffers arrive a few milliseconds
/// apart, so a gap far longer than that is not a quiet room — it is an audio
/// graph that stalled (or, on the phone's port, a webview the OS suspended).
/// Ageing the floor across the whole of such a gap would teleport it up to
/// whatever the first buffer back happens to be and cut the next utterance
/// short.
const MAX_FLOOR_AGE: Duration = Duration::from_millis(250);

/// How far above the room's own noise a buffer has to be to count as speech.
///
/// Deliberately the only loudness rule: an absolute "this loud is always
/// speech" level was tried and dropped, because steady noise above it (music,
/// air conditioning, a hot input) would refresh the speech clock on every
/// buffer and the session could never observe a pause. Relative to the floor,
/// steady noise *is* the floor and never counts.
const SPEECH_OVER_FLOOR: f32 = 3.0;

/// Floor under the noise floor. Digital silence would otherwise put the speech
/// threshold at zero and make the first faint buffer an utterance — and, since
/// the floor climbs back by multiplication, leave it stuck at zero for good.
const NOISE_FLOOR_MIN: f32 = 0.000_5;

/// A session's captured audio: mono f32 at the microphone's own sample rate.
pub(super) struct Pcm {
    /// The microphone's rate, which [`transcribe`] resamples from.
    rate: f64,
    /// Sample ceiling derived from [`MAX_CAPTURE_SECS`] and `rate`.
    limit: usize,
    samples: Mutex<Vec<f32>>,
    /// When capture began, which the timing below is measured from.
    start: Instant,
    /// When speech was last heard, as milliseconds since [`Pcm::start`] plus
    /// one; zero means never. One word rather than a flag and a timestamp, so
    /// the monitor can't read a flag that says "spoken" next to a timestamp
    /// that hasn't landed yet and take the whole session so far for a pause.
    last_speech: AtomicU64,
    /// When the previous buffer arrived, as microseconds since [`Pcm::start`],
    /// so the floor's rise can be paced in real time rather than in buffers —
    /// the tap's buffer size is the device's to choose, and the phone's port
    /// works in much smaller quanta.
    last_buffer_us: AtomicU64,
    /// This room on this microphone with nobody talking (`f32` bits). Adaptive
    /// because a threshold that suits a laptop's built-in mic is silence on a
    /// hot USB interface — and *recovering* (see [`settle_floor`]) because a
    /// floor that only ever fell would be pinned by the first quiet moment of
    /// the session and never rise back to the room.
    floor: AtomicU32,
    /// Set by `apple`'s `Sink::cancel` when the session is torn down, so the
    /// silence monitor stops polling a buffer nothing will fill again.
    closed: AtomicBool,
}

/// Is a buffer this loud speech, in a room whose noise floor is `floor`?
///
/// Pure, and the whole of the detector: a buffer has to stand out from the
/// room ([`SPEECH_OVER_FLOOR`]), and nothing under the engine's clip gate
/// counts — audio too quiet to transcribe can't be worth waiting for silence
/// after.
fn is_speech(rms: f32, floor: f32) -> bool {
    rms >= (floor * SPEECH_OVER_FLOOR).max(engine::MIN_RMS)
}

/// How much the floor may climb over `dt`, as a multiplier. Capped at
/// [`MAX_FLOOR_AGE`], so a stalled graph can't hand the floor a jump.
fn rise_factor(dt: Duration) -> f32 {
    10f32.powf(FLOOR_RISE_DB_PER_SEC * dt.min(MAX_FLOOR_AGE).as_secs_f32() / 20.0)
}

/// Fold a buffer's loudness into the noise floor: instantly down to anything
/// quieter, back up only at [`FLOOR_RISE_DB_PER_SEC`], and never above what is
/// actually being heard.
///
/// The lower bound matters on the way down and not only for tidiness: digital
/// silence would otherwise put the floor at zero, where a multiplicative rise
/// can never lift it again.
fn settle_floor(floor: f32, rms: f32, dt: Duration) -> f32 {
    let heard = rms.max(NOISE_FLOOR_MIN);
    if heard <= floor {
        heard
    } else {
        (floor * rise_factor(dt)).min(heard)
    }
}

/// Classify one buffer against the floor as it stood *before* this buffer, then
/// fold the buffer in. Judging a buffer against a floor it has just lowered
/// would make the first loud buffer of a session its own noise floor. `dt` is
/// how long since the previous buffer, which is what paces the floor's rise.
fn track(floor: f32, rms: f32, dt: Duration) -> (bool, f32) {
    (is_speech(rms, floor), settle_floor(floor, rms, dt))
}

/// Should the session end itself now? `last_speech` is when speech was last
/// heard, or `None` if it never was. `auto_stop` is the user's opt-out
/// (Settings › Dictation): off, the session runs until the mic button (or the
/// capture cap) ends it, however long the pauses. Pure so the thresholds are
/// testable without a microphone.
fn should_auto_stop(auto_stop: bool, last_speech: Option<Duration>, elapsed: Duration) -> bool {
    if !auto_stop {
        return false;
    }
    match last_speech {
        Some(last) => elapsed.saturating_sub(last) >= SILENCE_STOP,
        None => elapsed >= NO_SPEECH_TIMEOUT,
    }
}

impl Pcm {
    /// Render thread: fold one buffer's loudness into the speech tracker. The
    /// render thread is the only writer, so a load and a store are enough —
    /// no read-modify-write to lose.
    fn track_speech(&self, rms: f32) {
        let now = self.start.elapsed();
        let now_us = now.as_micros() as u64;
        let dt = Duration::from_micros(
            now_us.saturating_sub(self.last_buffer_us.swap(now_us, Ordering::Relaxed)),
        );
        let (speech, floor) = track(f32::from_bits(self.floor.load(Ordering::Relaxed)), rms, dt);
        self.floor.store(floor.to_bits(), Ordering::Relaxed);
        if speech {
            self.last_speech
                .store(now.as_millis() as u64 + 1, Ordering::Relaxed);
        }
    }

    /// Has the user spoken and then gone quiet (or never spoken at all)? Read
    /// by the silence monitor, off both the render and the main thread.
    pub(super) fn done_talking(&self) -> bool {
        let last = self
            .last_speech
            .load(Ordering::Relaxed)
            .checked_sub(1)
            .map(Duration::from_millis);
        should_auto_stop(super::auto_stop(), last, self.start.elapsed())
    }

    pub(super) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }
}

/// Build the accumulator and the tap block that fills it. The tap also feeds
/// `meter`, the session's level indicator, with the RMS it computes anyway.
pub(super) fn sink(format: &AVAudioFormat, meter: Arc<Meter>) -> Result<(Arc<Pcm>, Tap)> {
    // The input node hands out deinterleaved float32 on every OS that has
    // shipped. Reading a buffer in any other layout would be reading the wrong
    // memory, so refuse the session instead of guessing at it.
    if unsafe { format.commonFormat() } != AVAudioCommonFormat::PCMFormatFloat32
        || unsafe { format.isInterleaved() }
    {
        return Err(Error::Other(
            "This microphone's audio format isn't supported by local dictation. Switch the \
             dictation engine back to the system recognizer in Settings."
                .into(),
        ));
    }

    let rate = unsafe { format.sampleRate() };
    let limit = (rate * MAX_CAPTURE_SECS) as usize;
    let pcm = Arc::new(Pcm {
        rate,
        limit,
        // Sized to the cap up front: `append_mono` refuses to grow past `limit`,
        // so the render thread only ever writes into reserved capacity and a
        // reallocation — a copy of minutes of audio on the real-time thread —
        // can't happen. Reserving is cheap: the OS commits pages as they're
        // written, so an idle reservation of this size costs address space, not
        // memory.
        samples: Mutex::new(Vec::with_capacity(limit)),
        start: Instant::now(),
        last_speech: AtomicU64::new(0),
        last_buffer_us: AtomicU64::new(0),
        // The floor has to start above anything it will see; the first buffer
        // sets it, and can't be speech against itself.
        floor: AtomicU32::new(1.0f32.to_bits()),
        closed: AtomicBool::new(false),
    });

    let tap_pcm = pcm.clone();
    let tap = RcBlock::new(
        move |buffer: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
            if let Some(rms) = append_mono(&tap_pcm, unsafe { buffer.as_ref() }) {
                meter.record(rms);
            }
        },
    );
    Ok((pcm, tap))
}

/// Real-time audio thread: average the channels down, note how loud the result
/// was, and return it. The only other lock holder is the one-shot take in
/// [`transcribe`], so `try_lock` all but never fails — and dropping one buffer
/// beats blocking the render thread if it ever does. `None` when the buffer
/// was dropped, whichever reason.
///
/// The RMS rides along in the same pass because the silence monitor (and the
/// level meter) need it and the samples are already in registers here.
fn append_mono(pcm: &Pcm, buffer: &AVAudioPCMBuffer) -> Option<f32> {
    let frames = unsafe { buffer.frameLength() } as usize;
    let channels = unsafe { buffer.format().channelCount() } as usize;
    let data = unsafe { buffer.floatChannelData() };
    if data.is_null() || frames == 0 || channels == 0 {
        return None;
    }
    let mut samples = pcm.samples.try_lock()?;
    if samples.len() + frames > pcm.limit {
        return None;
    }
    let mut squares = 0.0f64;
    for frame in 0..frames {
        let mut sum = 0.0;
        for channel in 0..channels {
            // Deinterleaved (checked in `sink`): one pointer per channel, each
            // to `frameLength` contiguous samples.
            sum += unsafe { *(*data.add(channel)).as_ptr().add(frame) };
        }
        let mono = sum / channels as f32;
        squares += f64::from(mono) * f64::from(mono);
        samples.push(mono);
    }
    let rms = ((squares / frames as f64).sqrt()) as f32;
    pcm.track_speech(rms);
    Some(rms)
}

/// Take the session's audio and transcribe it with `model`. Consumes the
/// buffer: the session is already over by the time this runs.
pub(super) async fn transcribe(
    pcm: Arc<Pcm>,
    model: &'static super::whisper::models::WhisperModel,
) -> Result<String> {
    let rate = pcm.rate;
    let samples = std::mem::take(&mut *pcm.samples.lock());
    let samples = tokio::task::spawn_blocking(move || resample(rate, samples))
        .await
        .map_err(|e| Error::Other(format!("dictation: resampling task failed: {e}")))??;
    engine::transcribe(model, samples).await
}

/// Extra output capacity for the converter's priming, which can emit a little
/// more than the rate ratio implies.
const RESAMPLE_SLACK_FRAMES: u32 = 4096;

/// Convert mono `samples` at `rate` to the mono 16 kHz whisper.cpp wants.
///
/// Every ObjC handle here is created, used and dropped inside this call, so
/// nothing is shared across threads and nothing is live across an `.await` —
/// the same rule the recognizer path follows. Shared with `super::remote`,
/// whose audio arrives at whatever rate the phone captured.
pub(super) fn resample(rate: f64, samples: Vec<f32>) -> Result<Vec<f32>> {
    let target = f64::from(engine::SAMPLE_RATE);
    if rate == target || samples.is_empty() {
        return Ok(samples);
    }

    let from = standard_mono(rate)?;
    let to = standard_mono(target)?;
    let converter =
        unsafe { AVAudioConverter::initFromFormat_toFormat(AVAudioConverter::alloc(), &from, &to) }
            .ok_or_else(|| Error::Other("dictation: couldn't build the audio converter".into()))?;

    let frames = samples.len() as u32;
    let input = pcm_buffer(&from, frames)?;
    unsafe {
        std::ptr::copy_nonoverlapping(samples.as_ptr(), channel(&input)?.as_ptr(), samples.len());
        input.setFrameLength(frames);
    }

    let capacity = (samples.len() as f64 * target / rate).ceil() as u32 + RESAMPLE_SLACK_FRAMES;
    let output = pcm_buffer(&to, capacity)?;

    // The converter pulls input through a block rather than taking a buffer,
    // because a rate conversion may need more or less than it is given. There
    // is only ever one buffer to hand over here: serve it, then say the stream
    // has ended.
    let served = Cell::new(false);
    let feed = RcBlock::new(
        move |_packets: u32, status: NonNull<AVAudioConverterInputStatus>| -> *mut AVAudioBuffer {
            if served.replace(true) {
                unsafe { *status.as_ptr() = AVAudioConverterInputStatus::EndOfStream };
                return std::ptr::null_mut();
            }
            unsafe { *status.as_ptr() = AVAudioConverterInputStatus::HaveData };
            Retained::as_ptr(&input).cast_mut().cast()
        },
    );

    let mut error = None;
    let status = unsafe {
        converter.convertToBuffer_error_withInputFromBlock(
            &output,
            Some(&mut error),
            RcBlock::as_ptr(&feed),
        )
    };
    if status == AVAudioConverterOutputStatus::Error {
        let message = error.map_or_else(
            || "unknown error".to_string(),
            |e| e.localizedDescription().to_string(),
        );
        return Err(Error::Other(format!(
            "dictation: couldn't resample the recording: {message}"
        )));
    }

    // Whatever the status, `frameLength` is what was actually converted.
    let converted = unsafe { output.frameLength() } as usize;
    let mut out = vec![0.0; converted];
    unsafe {
        std::ptr::copy_nonoverlapping(channel(&output)?.as_ptr(), out.as_mut_ptr(), converted)
    };
    Ok(out)
}

/// Deinterleaved float32, one channel, at `rate`.
pub(super) fn standard_mono(rate: f64) -> Result<Retained<AVAudioFormat>> {
    unsafe {
        AVAudioFormat::initStandardFormatWithSampleRate_channels(AVAudioFormat::alloc(), rate, 1)
    }
    .ok_or_else(|| {
        Error::Other(format!(
            "dictation: {rate} Hz mono isn't a valid audio format"
        ))
    })
}

pub(super) fn pcm_buffer(
    format: &AVAudioFormat,
    frames: u32,
) -> Result<Retained<AVAudioPCMBuffer>> {
    unsafe {
        AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(AVAudioPCMBuffer::alloc(), format, frames)
    }
    .ok_or_else(|| Error::Other("dictation: couldn't allocate an audio buffer".into()))
}

/// The one channel's samples. Null for a buffer that isn't float32, which the
/// formats above always are.
pub(super) fn channel(buffer: &AVAudioPCMBuffer) -> Result<NonNull<f32>> {
    NonNull::new(unsafe { buffer.floatChannelData() })
        .map(|data| unsafe { *data.as_ptr() })
        .ok_or_else(|| Error::Other("dictation: audio buffer has no float samples".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resampler is the one part of the capture path that can be exercised
    /// without a microphone: a 440 Hz tone is well inside the 8 kHz band 16 kHz
    /// can carry, so it must come out the same length in seconds and the same
    /// loudness.
    #[test]
    fn resample_keeps_a_tone_that_fits() {
        let rate = 48_000.0;
        let input: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin())
            .collect();

        let out = resample(rate, input).unwrap();

        let expected = engine::SAMPLE_RATE as i64;
        assert!(
            (out.len() as i64 - expected).abs() < expected / 20,
            "one second in, {} samples out",
            out.len()
        );
        let rms = (out
            .iter()
            .map(|s| f64::from(*s) * f64::from(*s))
            .sum::<f64>()
            / out.len() as f64)
            .sqrt();
        assert!(rms > 0.5, "tone was lost: rms {rms}");
    }

    /// One tap buffer, near enough: 1024 frames at 44.1 kHz.
    const BUFFER: Duration = Duration::from_millis(23);

    /// Walk a sequence of buffer RMS values through the detector the way the
    /// render thread would, and report which of them counted as speech. The
    /// buffers arrive one [`BUFFER`] apart, which is what paces the floor.
    fn detect(sequence: &[f32]) -> Vec<bool> {
        let mut floor = 1.0;
        sequence
            .iter()
            .map(|rms| {
                let (speech, next) = track(floor, *rms, BUFFER);
                floor = next;
                speech
            })
            .collect()
    }

    /// `seconds` of buffers at a steady `rms`, for the cases about what a room
    /// sounds like over time rather than what one buffer means.
    fn steady(seconds: f32, rms: f32) -> Vec<f32> {
        vec![rms; (seconds / BUFFER.as_secs_f32()) as usize]
    }

    /// The shape every session has: a quiet room, an utterance, then quiet
    /// again. Only the utterance may count, and the trailing silence must not —
    /// that is what ends the session.
    #[test]
    fn detects_speech_between_silences() {
        let mut sequence = vec![0.001; 5];
        sequence.extend([0.05; 10]);
        sequence.extend([0.001; 5]);

        let speech = detect(&sequence);

        assert_eq!(speech[..5], [false; 5], "quiet room read as speech");
        assert_eq!(speech[5..15], [true; 10], "speech missed");
        assert_eq!(speech[15..], [false; 5], "the pause never arrives");
    }

    /// Talking from the very first buffer — clicking mid-sentence — sets the
    /// floor to the voice itself, so nothing counts until the first gap between
    /// words lowers it; from then on the speech does.
    #[test]
    fn speech_from_the_first_buffer_counts_after_the_first_gap() {
        assert_eq!(
            detect(&[0.05, 0.05, 0.002, 0.05, 0.05, 0.002]),
            [false, false, false, true, true, false]
        );
    }

    /// Steady noise of any level is the room, not a voice: it must never keep
    /// the session open, however loud.
    #[test]
    fn steady_noise_is_never_speech() {
        assert_eq!(detect(&[0.02; 6]), [false; 6]);
        assert_eq!(detect(&[0.2; 6]), [false; 6]);
    }

    /// A hot input's noise is louder than a quiet one's speech, so below the
    /// absolute level the threshold follows the room rather than a fixed
    /// number.
    #[test]
    fn floor_adapts_to_the_room() {
        assert_eq!(detect(&[0.01, 0.01, 0.015]), [false, false, false]);
        assert_eq!(detect(&[0.0005, 0.0005, 0.005]), [false, false, true]);
    }

    /// A muted or unplugged input is all zeroes: the adaptive threshold
    /// collapses to nothing there, and the lower bound has to hold.
    #[test]
    fn silence_is_never_speech() {
        assert_eq!(detect(&[0.0; 4]), [false; 4]);
        assert!(!is_speech(engine::MIN_RMS / 2.0, 0.0));
    }

    /// The regression this whole tracker exists for. One near-silent buffer —
    /// the mic warming up, a breath between words — used to pull the floor to
    /// [`NOISE_FLOOR_MIN`] and leave it there for the rest of the session,
    /// because the floor was a minimum that could only ever fall. From then on
    /// the threshold was the absolute lower bound, ordinary room tone cleared
    /// it on every buffer, and the pause the session was waiting for could
    /// never arrive: dictation ran until the user clicked stop.
    ///
    /// Room tone at 0.003 is about −50 dBFS: a fan, an air conditioner, traffic
    /// through a window.
    #[test]
    fn a_quiet_dip_does_not_pin_the_floor() {
        let mut sequence = vec![0.0];
        sequence.extend(steady(3.0, 0.003));

        let speech = detect(&sequence);

        // The floor needs a moment to climb back to the room, so the tone may
        // read as speech at first. What matters is that it stops.
        let recovered = speech.iter().rposition(|s| *s).unwrap_or(0);
        let took = BUFFER.as_secs_f32() * recovered as f32;
        assert!(
            took < SILENCE_STOP.as_secs_f32(),
            "room tone still read as speech after {took:.1}s — the floor is pinned again"
        );
    }

    /// A pause has to survive the floor's recovery: the session may not be held
    /// open by the room, however long it goes on.
    #[test]
    fn steady_room_tone_after_speech_still_ends_the_session() {
        let mut sequence = vec![0.0];
        sequence.extend(steady(1.0, 0.05));
        sequence.extend(steady(6.0, 0.003));

        let speech = detect(&sequence);
        let last = speech.iter().rposition(|s| *s).unwrap();
        let quiet_for = BUFFER.as_secs_f32() * (speech.len() - 1 - last) as f32;

        assert!(
            quiet_for >= SILENCE_STOP.as_secs_f32(),
            "only {quiet_for:.1}s of quiet observed; the session would not stop"
        );
    }

    /// The other side of the tradeoff: the floor must not climb fast enough to
    /// swallow the voice that is lifting it, or a long sentence would cut out
    /// mid-word.
    ///
    /// Speech, not a tone — words with the gaps between them, which is what
    /// keeps pulling the floor back down. A held, gapless level is deliberately
    /// *not* speech here however loud it is (see
    /// [`steady_noise_is_never_speech`]), so a constant RMS would be testing
    /// the opposite rule.
    #[test]
    fn a_long_sentence_is_heard_all_the_way_through() {
        let mut sequence = vec![0.001];
        for _ in 0..18 {
            sequence.extend(steady(0.30, 0.05));
            sequence.extend(steady(0.12, 0.004));
        }

        let speech = detect(&sequence);
        let words = speech.len() - 14;

        assert!(
            speech[words..].iter().any(|s| *s),
            "after {:.0}s the floor had caught up with the voice and the session \
             would have stopped mid-sentence",
            BUFFER.as_secs_f32() * words as f32
        );
    }

    /// Instantly down, gradually up, and never past what is actually being
    /// heard — a floor that overshot the room would stop hearing it.
    #[test]
    fn the_floor_falls_at_once_and_climbs_slowly() {
        assert_eq!(settle_floor(0.05, 0.001, BUFFER), 0.001);

        let climbed = settle_floor(NOISE_FLOOR_MIN, 0.05, BUFFER);
        assert!(climbed > NOISE_FLOOR_MIN, "the floor never recovers");
        assert!(climbed < 0.001, "the floor jumped rather than climbed");

        // A whole second of it still can't pass the level it is climbing towards.
        let mut floor = NOISE_FLOOR_MIN;
        for _ in 0..43 {
            floor = settle_floor(floor, 0.002, BUFFER);
        }
        assert!(floor <= 0.002, "the floor overshot the room: {floor}");
    }

    /// A suspended webview or a stalled graph hands the tracker one buffer with
    /// minutes on it. Ageing the floor across all of that would put it at the
    /// level of whatever came back and swallow the utterance in progress.
    #[test]
    fn a_stalled_graph_does_not_teleport_the_floor() {
        let stalled = settle_floor(NOISE_FLOOR_MIN, 0.05, Duration::from_secs(120));
        let capped = settle_floor(NOISE_FLOOR_MIN, 0.05, MAX_FLOOR_AGE);

        assert_eq!(stalled, capped);
        assert!(is_speech(0.05, stalled), "the voice was lost to the stall");
    }

    #[test]
    fn auto_stops_on_a_pause_but_not_before_speech() {
        let long = NO_SPEECH_TIMEOUT + Duration::from_secs(1);
        let spoke_at = Duration::from_secs(1);
        assert!(!should_auto_stop(
            true,
            Some(spoke_at),
            spoke_at + SILENCE_STOP / 2
        ));
        assert!(should_auto_stop(
            true,
            Some(spoke_at),
            spoke_at + SILENCE_STOP
        ));
        // Speech that started after a long wait counts from when it was heard,
        // not from the start of the session.
        assert!(!should_auto_stop(true, Some(long), long + SILENCE_STOP / 2));
        // Never spoke: the pause is the whole session, and only the longer
        // deadline ends it.
        assert!(!should_auto_stop(true, None, SILENCE_STOP * 2));
        assert!(should_auto_stop(true, None, long));
    }

    /// With the opt-out off, no pause ends the session — not after speech, and
    /// not the never-spoke deadline either.
    #[test]
    fn never_auto_stops_when_turned_off() {
        let long = NO_SPEECH_TIMEOUT + Duration::from_secs(1);
        assert!(!should_auto_stop(false, Some(Duration::from_secs(1)), long));
        assert!(!should_auto_stop(false, None, long));
    }

    #[test]
    fn resample_passes_matching_rates_through() {
        let samples = vec![0.25; 100];
        assert_eq!(
            resample(f64::from(engine::SAMPLE_RATE), samples.clone()).unwrap(),
            samples
        );
    }
}
