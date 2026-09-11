//! How loud the microphone is right now, and what that loudness means: the
//! composer's listening indicator, and the pause that ends a session.
//!
//! The tap on the render thread folds each buffer's RMS into a [`Meter`] — a
//! handful of relaxed atomic stores, nothing else — and two tasks off that
//! thread read it back: [`watch`] samples the level every [`LEVEL_POLL`] and
//! emits `dictation:level`, and `apple::watch_for_silence` asks every
//! [`SILENCE_POLL`] whether the user has finished talking. Both engines share
//! the meter, which is what makes hands-free stop the same feature on both:
//! the local engine already computes the RMS for the pass that copies the
//! samples and hands it over; Apple's path reads the buffer once more on the
//! way to the bridge, because the analyzer takes the buffer whole and tells us
//! nothing about a pause until it has decided an utterance ended.
//!
//! One measurement serving both readers is deliberate — the bars and the
//! silence detector have to agree about what silence is, or a session that
//! won't end looks like a frozen UI rather than a mic that thinks you're still
//! speaking (see `the_bars_start_where_speech_can`).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use objc2_avf_audio::{AVAudioCommonFormat, AVAudioFormat, AVAudioPCMBuffer};
use tauri::AppHandle;

use super::whisper::engine;

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

/// How long a pause has to last, once something has been said, for the session
/// to end itself — long enough to think mid-sentence, short enough that the
/// text lands while the user is still looking at the composer.
///
/// This and the two constants below are the whole of the hands-free policy. The
/// Settings opt-out (`dictation_auto_stop`) gates the monitor that reads them
/// rather than changing them — see [`should_auto_stop`].
const SILENCE_STOP: Duration = Duration::from_secs(2);

/// How long a session in which nothing was ever said stays open: the user
/// clicked the mic and walked away. On the local engine the clip has no speech
/// in it, so the engine's gate answers empty; on Apple's the analyzer has
/// nothing to finalize. Either way the session just ends.
const NO_SPEECH_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the monitor looks. Well under [`SILENCE_STOP`], and cheap: three
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

/// The latest buffer's loudness, written by the render thread and read by the
/// emitter task and the silence monitor.
pub(super) struct Meter {
    /// RMS of the most recent buffer, as `f32` bits.
    rms: AtomicU32,
    /// Whether the tap's buffers can be read as deinterleaved float32 — the
    /// layout every shipped input node delivers. A meter on any other layout
    /// simply reports silence rather than read the wrong memory.
    readable: bool,
    /// Set when the mic is closed, so the readers stop.
    closed: AtomicBool,
    /// What the same measurements say about whether anyone is talking.
    speech: SpeechTracker,
}

/// The hands-free half of the meter: whether speech has been heard, and when it
/// last was.
///
/// Its own type rather than four more fields on [`Meter`] because the two
/// answers have nothing to do with each other — a level is the last buffer, a
/// pause is the whole session so far — and because taking the clock as a
/// parameter is what makes the policy above testable without a microphone or a
/// wall clock. [`Meter`] supplies the clock, and the readability gate the
/// tracker has no way to know about.
struct SpeechTracker {
    /// When capture began, which the timing below is measured from.
    start: Instant,
    /// When speech was last heard, as milliseconds since [`SpeechTracker::start`]
    /// plus one; zero means never. One word rather than a flag and a timestamp,
    /// so the monitor can't read a flag that says "spoken" next to a timestamp
    /// that hasn't landed yet and take the whole session so far for a pause.
    last_speech: AtomicU64,
    /// When the previous buffer arrived, as microseconds since
    /// [`SpeechTracker::start`], so the floor's rise can be paced in real time
    /// rather than in buffers — the tap's buffer size is the device's to choose,
    /// and the phone's port works in much smaller quanta.
    last_buffer_us: AtomicU64,
    /// This room on this microphone with nobody talking (`f32` bits). Adaptive
    /// because a threshold that suits a laptop's built-in mic is silence on a
    /// hot USB interface — and *recovering* (see [`settle_floor`]) because a
    /// floor that only ever fell would be pinned by the first quiet moment of
    /// the session and never rise back to the room.
    floor: AtomicU32,
}

impl SpeechTracker {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            last_speech: AtomicU64::new(0),
            last_buffer_us: AtomicU64::new(0),
            // The floor has to start above anything it will see; the first
            // buffer sets it, and can't be speech against itself.
            floor: AtomicU32::new(1.0f32.to_bits()),
        }
    }

    /// How long this session has been running.
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Render thread: fold one buffer's loudness in, `now` being how far into
    /// the session it arrived. The render thread is the only writer, so a load
    /// and a store are enough — no read-modify-write to lose.
    fn record(&self, rms: f32, now: Duration) {
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

    /// Has the user spoken and then gone quiet (or never spoken at all), as of
    /// `elapsed` into the session?
    fn done(&self, auto_stop: bool, elapsed: Duration) -> bool {
        let last = self
            .last_speech
            .load(Ordering::Relaxed)
            .checked_sub(1)
            .map(Duration::from_millis);
        should_auto_stop(auto_stop, last, elapsed)
    }
}

impl Meter {
    pub(super) fn new(format: &AVAudioFormat) -> Arc<Self> {
        let readable = unsafe { format.commonFormat() } == AVAudioCommonFormat::PCMFormatFloat32
            && !unsafe { format.isInterleaved() };
        Arc::new(Self {
            rms: AtomicU32::new(0.0f32.to_bits()),
            readable,
            closed: AtomicBool::new(false),
            speech: SpeechTracker::new(),
        })
    }

    /// Render thread: note one buffer's RMS, for the bars and the pause
    /// detector alike.
    pub(super) fn record(&self, rms: f32) {
        self.rms.store(rms.to_bits(), Ordering::Relaxed);
        self.speech.record(rms, self.speech.elapsed());
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

    /// Has the user spoken and then gone quiet? Read by the silence monitor,
    /// off both the render and the main thread.
    pub(super) fn done_talking(&self) -> bool {
        self.verdict(super::auto_stop(), self.speech.elapsed())
    }

    /// The same question with the setting and the clock supplied, which is what
    /// makes it testable.
    ///
    /// An unreadable meter never answers yes, and that is the whole reason this
    /// gate lives here rather than in the tracker. Nothing feeds a meter it
    /// can't read, so its tracker would see a session in which nobody ever
    /// spoke and end it on [`NO_SPEECH_TIMEOUT`] — cutting off a user who is
    /// talking perfectly audibly into an exotic input format. Silent bars are a
    /// cosmetic loss; a session that hangs up after ten seconds is not. Only
    /// Apple's path can get here: [`super::capture::sink`] refuses a
    /// non-float32 format outright rather than open the mic at all.
    fn verdict(&self, auto_stop: bool, elapsed: Duration) -> bool {
        self.readable && self.speech.done(auto_stop, elapsed)
    }

    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }

    pub(super) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
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

    use objc2::rc::Retained;
    use objc2::AllocAnyThread;

    use crate::dictation::capture;

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

    /// The format the input node actually delivers, and the one both sinks are
    /// built on.
    fn readable_format() -> Retained<AVAudioFormat> {
        capture::standard_mono(44_100.0).unwrap()
    }

    /// Something no shipped input node produces, but which the meter has to
    /// survive being handed: interleaved 16-bit.
    fn unreadable_format() -> Retained<AVAudioFormat> {
        unsafe {
            AVAudioFormat::initWithCommonFormat_sampleRate_channels_interleaved(
                AVAudioFormat::alloc(),
                AVAudioCommonFormat::PCMFormatInt16,
                44_100.0,
                1,
                true,
            )
        }
        .unwrap()
    }

    /// A mono buffer of `frames` samples at a constant amplitude, so its RMS is
    /// exactly that amplitude — what Apple's tap hands `record_buffer`.
    fn tone(amplitude: f32, frames: u32) -> Retained<AVAudioPCMBuffer> {
        let format = readable_format();
        let buffer = capture::pcm_buffer(&format, frames).unwrap();
        let data = capture::channel(&buffer).unwrap();
        unsafe {
            for frame in 0..frames as usize {
                *data.as_ptr().add(frame) = amplitude;
            }
            buffer.setFrameLength(frames);
        }
        buffer
    }

    /// The sequence both paths are driven with: a quiet room, then a voice. The
    /// first buffer sets the floor, so only the loud ones may count.
    const ROOM_THEN_VOICE: [f32; 4] = [0.001, 0.001, 0.05, 0.05];

    /// The point of the move. Apple's tap measures the buffer itself and the
    /// local engine's hands over the RMS from the pass it was making anyway —
    /// two ways into the same tracker, which must reach the same verdict, or
    /// hands-free stop would mean something different per engine.
    #[test]
    fn both_paths_reach_the_same_verdict() {
        let format = readable_format();
        let whisper = Meter::new(&format);
        let apple = Meter::new(&format);

        for rms in ROOM_THEN_VOICE {
            // What `capture::sink`'s tap does with the RMS it already computed.
            whisper.record(rms);
            // What `apple::speech_sink`'s tap does with a buffer it is only
            // passing through.
            apple.record_buffer(&tone(rms, 512));
        }

        // Measured the same, so the bars read the same.
        assert!((whisper.level() - apple.level()).abs() < 1e-4);
        // And heard the same, at every point in the pause that follows. The
        // exact threshold is deliberately not one of them: the two meters were
        // fed by a real clock a millisecond or two apart, and the pure
        // predicate is where the boundary itself is pinned down.
        for elapsed in [
            Duration::ZERO,
            SILENCE_STOP / 2,
            SILENCE_STOP * 2,
            NO_SPEECH_TIMEOUT * 2,
        ] {
            assert_eq!(
                whisper.verdict(true, elapsed),
                apple.verdict(true, elapsed),
                "the two paths disagree {elapsed:?} in"
            );
        }
        // Not just agreeing on "no": the voice was heard, and the pause after
        // it ends the session.
        assert!(!apple.verdict(true, Duration::ZERO));
        assert!(apple.verdict(true, SILENCE_STOP * 2));
    }

    /// The failure mode the readability gate exists for. Nothing feeds a meter
    /// it can't read, so its tracker sees a session in which nobody ever spoke
    /// — and the never-spoke deadline would hang up on a user who is talking
    /// perfectly audibly, ten seconds in, with no way to tell why.
    #[test]
    fn an_unreadable_meter_never_reports_a_pause() {
        let meter = Meter::new(&unreadable_format());

        for _ in 0..100 {
            meter.record_buffer(&tone(0.05, 512));
        }

        assert_eq!(meter.level(), 0.0, "an unreadable buffer was measured");
        for minutes in 0..10 {
            let elapsed = NO_SPEECH_TIMEOUT * (1 + minutes * 6);
            assert!(
                !meter.verdict(true, elapsed),
                "an unreadable meter ended the session after {elapsed:?}"
            );
        }
    }

    /// The opt-out reaches the meter, not only the pure predicate — a session
    /// with auto-stop off runs until the user ends it however long the pause.
    #[test]
    fn the_opt_out_holds_at_the_meter() {
        let meter = Meter::new(&readable_format());
        for rms in ROOM_THEN_VOICE {
            meter.record(rms);
        }

        assert!(meter.verdict(true, SILENCE_STOP * 2));
        assert!(!meter.verdict(false, SILENCE_STOP * 2));
        assert!(!meter.verdict(false, NO_SPEECH_TIMEOUT * 10));
    }
}
