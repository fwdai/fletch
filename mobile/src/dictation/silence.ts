// Hands-free dictation on the phone: has the user spoken and then gone quiet?
//
// A port of the detector in `src-tauri/src/dictation/capture.rs` — the same
// rules and the same constants, because the Mac transcribes both paths and a
// session that ended at a different moment on the phone would read as a bug.
// The host's monitor can't do this job for us: it never sees the frames, only
// the chunks the phone chose to send.
//
// Numbers and an injectable clock, nothing else: no audio, no timers, no
// React. `capture.ts` feeds it and polls it.

/** How long a pause has to last, once something has been said, for the session
 *  to end itself — long enough to think mid-sentence, short enough that the
 *  text lands while the user is still looking at the composer. */
export const SILENCE_STOP_MS = 2_000;

/** How long a session in which nothing was ever said stays open: the user
 *  tapped the mic and put the phone down. The clip has no speech in it, so the
 *  host's silence gate answers empty and the session just ends. */
export const NO_SPEECH_TIMEOUT_MS = 10_000;

/** How often the owner looks. Well under `SILENCE_STOP_MS`, and cheap. */
export const SILENCE_POLL_MS = 100;

/** How far above the room's own noise a buffer has to be to count as speech.
 *
 *  Deliberately the only loudness rule: an absolute "this loud is always
 *  speech" level was tried on the desktop and dropped, because steady noise
 *  above it (music, a train) would refresh the speech clock on every buffer and
 *  the session could never observe a pause. Relative to the floor, steady noise
 *  *is* the floor and never counts. */
const SPEECH_OVER_FLOOR = 3;

/** Floor under the noise floor. Digital silence would otherwise put the speech
 *  threshold at zero and make the first faint buffer an utterance. */
const NOISE_FLOOR_MIN = 0.000_5;

/** The host's clip gate (`whisper::engine::MIN_RMS`): audio too quiet for the
 *  Mac to transcribe can't be worth waiting for silence after. */
export const MIN_RMS = 0.002;

/** One render quantum's loudness. */
export function rms(frames: Float32Array): number {
  if (frames.length === 0) return 0;
  let squares = 0;
  for (const sample of frames) squares += sample * sample;
  return Math.sqrt(squares / frames.length);
}

/** Is a buffer this loud speech, in a room whose noise floor is `floor`? Pure,
 *  and the whole of the detector. */
export function isSpeech(level: number, floor: number): boolean {
  return level >= Math.max(floor * SPEECH_OVER_FLOOR, MIN_RMS);
}

/** Fold a buffer's loudness into the noise floor: the running minimum, never
 *  below `NOISE_FLOOR_MIN`. */
export function settleFloor(floor: number, level: number): number {
  return Math.min(floor, Math.max(level, NOISE_FLOOR_MIN));
}

/** Classify one buffer against the floor as it stood *before* this buffer, then
 *  fold the buffer in. Judging a buffer against a floor it has just lowered
 *  would make the first loud buffer of a session its own noise floor. */
export function track(floor: number, level: number): [speech: boolean, floor: number] {
  return [isSpeech(level, floor), settleFloor(floor, level)];
}

/** Should the session end itself now? `lastSpeech` is how far into the session
 *  speech was last heard, in ms, or null if it never was. Pure, so the
 *  thresholds are testable without a microphone. */
export function shouldAutoStop(lastSpeech: number | null, elapsed: number): boolean {
  if (lastSpeech === null) return elapsed >= NO_SPEECH_TIMEOUT_MS;
  return elapsed - lastSpeech >= SILENCE_STOP_MS;
}

/** The detector with the state a session accumulates: the room's noise floor
 *  and when speech was last heard. `now` is injected so tests need no timers. */
export class SilenceMonitor {
  /** A running minimum has to start above anything it will see; the first
   *  buffer sets it, and can't be speech against itself. */
  private floor = 1;
  /** Milliseconds into the session, or null for never. */
  private lastSpeech: number | null = null;
  private readonly start: number;

  constructor(private readonly now: () => number = Date.now) {
    this.start = now();
  }

  /** Fold one render quantum's frames into the tracker. Returns the quantum's
   *  RMS, which the level meter displays — one pass over the samples for both. */
  hear(frames: Float32Array): number {
    const level = rms(frames);
    const [speech, floor] = track(this.floor, level);
    this.floor = floor;
    if (speech) this.lastSpeech = this.now() - this.start;
    return level;
  }

  /** Has the user spoken and then gone quiet (or never spoken at all)? */
  doneTalking(): boolean {
    return shouldAutoStop(this.lastSpeech, this.now() - this.start);
  }
}
