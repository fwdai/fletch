import { describe, expect, it } from "vitest";
import {
  isSpeech,
  MIN_RMS,
  NO_SPEECH_TIMEOUT_MS,
  rms,
  SILENCE_STOP_MS,
  SilenceMonitor,
  settleFloor,
  shouldAutoStop,
  track,
} from "../src/dictation/silence";

const repeat = <T>(times: number, value: T): T[] => Array.from({ length: times }, () => value);

/** How far apart the buffers in these cases are taken to be: one render quantum
 *  of 128 frames at 48 kHz, which is what the phone actually hands the capture
 *  path. The floor's climb is a rate per second, so the cases below need a gap
 *  to age it by — and at this gap a handful of buffers moves it by hundredths
 *  of a decibel, which is why the pre-existing expectations are untouched. */
const QUANTUM_MS = 3;

/** How many quanta fill `ms`, for the cases that have to run long enough for
 *  the floor to travel. */
const quanta = (ms: number) => Math.round(ms / QUANTUM_MS);

/** Walk a sequence of buffer RMS values through the detector the way the
 *  capture path would, and report which of them counted as speech. Mirrors the
 *  `detect` helper in `src-tauri/src/dictation/capture.rs`, whose cases these
 *  are: the two detectors have to agree buffer for buffer. */
function detect(sequence: number[]): boolean[] {
  let floor = 1;
  return sequence.map((level) => {
    const [speech, next] = track(floor, level, QUANTUM_MS);
    floor = next;
    return speech;
  });
}

describe("silence detector", () => {
  /** The shape every session has: a quiet room, an utterance, then quiet
   *  again. Only the utterance may count, and the trailing silence must not —
   *  that is what ends the session. */
  it("detects speech between silences", () => {
    const sequence = [...repeat(5, 0.001), ...repeat(10, 0.05), ...repeat(5, 0.001)];

    const speech = detect(sequence);

    expect(speech.slice(0, 5)).toEqual(repeat(5, false));
    expect(speech.slice(5, 15)).toEqual(repeat(10, true));
    expect(speech.slice(15)).toEqual(repeat(5, false));
  });

  /** Talking from the very first buffer — tapping mid-sentence — sets the floor
   *  to the voice itself, so nothing counts until the first gap between words
   *  lowers it; from then on the speech does. */
  it("counts speech from the first buffer after the first gap", () => {
    expect(detect([0.05, 0.05, 0.002, 0.05, 0.05, 0.002])).toEqual([
      false,
      false,
      false,
      true,
      true,
      false,
    ]);
  });

  /** Steady noise of any level is the room, not a voice: it must never keep the
   *  session open, however loud. */
  it("never reads steady noise as speech", () => {
    expect(detect(repeat(6, 0.02))).toEqual(repeat(6, false));
    expect(detect(repeat(6, 0.2))).toEqual(repeat(6, false));
  });

  /** A hot input's noise is louder than a quiet one's speech, so below the
   *  absolute level the threshold follows the room rather than a fixed
   *  number. */
  it("adapts the floor to the room", () => {
    expect(detect([0.01, 0.01, 0.015])).toEqual([false, false, false]);
    expect(detect([0.0005, 0.0005, 0.005])).toEqual([false, false, true]);
  });

  /** A muted mic is all zeroes: the adaptive threshold collapses to nothing
   *  there, and the lower bound has to hold. */
  it("never reads digital silence as speech", () => {
    expect(detect(repeat(4, 0))).toEqual(repeat(4, false));
    expect(isSpeech(MIN_RMS / 2, 0)).toBe(false);
  });

  /** The bug this detector shipped with: the floor was a running minimum over
   *  the session, so the one near-silent buffer below pinned it at its lower
   *  bound and left every later buffer of ordinary room tone sitting three
   *  times above it. The room read as speech on every buffer, forever, and no
   *  session could ever see the pause that ends it. The floor now climbs back
   *  out of the pin, so the tone stops counting about a second in. */
  it("stops calling room tone speech after a quiet buffer pins the floor", () => {
    const tone = 0.003; // about −50 dBFS: a quiet room, not a voice.
    const speech = detect([0, ...repeat(quanta(3_000), tone)]);

    expect(speech[1]).toBe(true); // the pin does still fool the first buffers,
    expect(speech.slice(1 + quanta(2_000)).some(Boolean)).toBe(false); // but not for long.
  });

  it("auto-stops on a pause but not before speech", () => {
    const long = NO_SPEECH_TIMEOUT_MS + 1_000;
    const spokeAt = 1_000;
    expect(shouldAutoStop(spokeAt, spokeAt + SILENCE_STOP_MS / 2)).toBe(false);
    expect(shouldAutoStop(spokeAt, spokeAt + SILENCE_STOP_MS)).toBe(true);
    // Speech that started after a long wait counts from when it was heard, not
    // from the start of the session.
    expect(shouldAutoStop(long, long + SILENCE_STOP_MS / 2)).toBe(false);
    // Never spoke: the pause is the whole session, and only the longer deadline
    // ends it.
    expect(shouldAutoStop(null, SILENCE_STOP_MS * 2)).toBe(false);
    expect(shouldAutoStop(null, long)).toBe(true);
  });

  /** The other half of the fix, on its own: the floor is not a ratchet. It
   *  climbs out of a pin towards whatever it is hearing, and stops there —
   *  going above the room would put the speech threshold out of a voice's reach
   *  and deafen the session instead of deafening the pause. */
  it("lets the floor climb back out of a pin, never past the room", () => {
    const room = 0.01;
    const pinned = settleFloor(1, 0, QUANTUM_MS); // one silent buffer, floor at its minimum.

    let floor = pinned;
    for (let i = 0; i < quanta(6_000); i++) {
      const next = settleFloor(floor, room, QUANTUM_MS);
      expect(next).toBeGreaterThanOrEqual(floor);
      expect(next).toBeLessThanOrEqual(room);
      floor = next;
    }

    expect(floor).toBeGreaterThan(pinned);
    expect(floor).toBe(room);
  });

  it("measures a buffer's loudness", () => {
    expect(rms(new Float32Array(0))).toBe(0);
    expect(rms(Float32Array.from([0.5, -0.5]))).toBeCloseTo(0.5);
    expect(rms(Float32Array.from([1, -1, 0, 0]))).toBeCloseTo(Math.SQRT1_2);
  });
});

describe("SilenceMonitor", () => {
  /** A monitor on a clock the test winds by hand, so the deadlines are checked
   *  without waiting for them. */
  function monitor() {
    let now = 1_000;
    const m = new SilenceMonitor(() => now);
    const quantum = (level: number) => m.hear(new Float32Array(128).fill(level));
    return {
      /** Feed one buffer at `level`, `ms` after the previous one. */
      hear(level: number, ms: number) {
        now += ms;
        quantum(level);
      },
      wait(ms: number) {
        now += ms;
      },
      done: () => m.doneTalking(),
    };
  }

  it("ends a session two seconds after the talking stops", () => {
    const m = monitor();
    // A quiet room first, so the floor is the room and not the voice.
    m.hear(0.001, 20);
    m.hear(0.05, 20);
    expect(m.done()).toBe(false);

    m.wait(SILENCE_STOP_MS - 1);
    expect(m.done()).toBe(false);
    m.wait(1);
    expect(m.done()).toBe(true);
  });

  /** The fixture feeds a real stream: quanta a few milliseconds apart, words
   *  with the gaps between them. It used to hand the monitor one quantum every
   *  `SILENCE_STOP_MS - 100`, which no microphone does — that only passed while
   *  the floor ignored time entirely, and against a floor that ages it reads as
   *  1.9 seconds of room per buffer. Words and gaps are also what the rule
   *  needs: a held, gapless level is deliberately the room and not a voice (see
   *  "never reads steady noise as speech"). */
  it("holds a session open while the speech keeps coming", () => {
    const m = monitor();
    m.hear(0.001, QUANTUM_MS);
    for (let word = 0; word < 18; word++) {
      for (let i = 0; i < quanta(300); i++) m.hear(0.05, QUANTUM_MS);
      for (let i = 0; i < quanta(120); i++) m.hear(0.004, QUANTUM_MS);
      expect(m.done()).toBe(false);
    }
  });

  /** The bug as a session rather than as a sequence: a dip at the top (the mic
   *  warming up), a sentence, and then the room the user is sitting in. The
   *  dip used to pin the floor low enough that the room refreshed the speech
   *  clock on every buffer and the mic stayed open until the user tapped it. */
  it("ends a session whose tail is room tone, not silence", () => {
    const m = monitor();
    m.hear(0, QUANTUM_MS);
    for (let i = 0; i < quanta(500); i++) m.hear(0.05, QUANTUM_MS);
    expect(m.done()).toBe(false);

    for (let i = 0; i < quanta(5_000); i++) m.hear(0.003, QUANTUM_MS);
    expect(m.done()).toBe(true);
  });

  it("ends a session nobody ever spoke in on the longer deadline", () => {
    const m = monitor();
    m.hear(0, 20);
    m.wait(NO_SPEECH_TIMEOUT_MS - 20 - 1);
    expect(m.done()).toBe(false);
    m.wait(1);
    expect(m.done()).toBe(true);
  });
});
