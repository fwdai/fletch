import { describe, expect, it } from "vitest";
import {
  isSpeech,
  MIN_RMS,
  NO_SPEECH_TIMEOUT_MS,
  rms,
  SILENCE_STOP_MS,
  SilenceMonitor,
  shouldAutoStop,
  track,
} from "../src/dictation/silence";

const repeat = <T>(times: number, value: T): T[] => Array.from({ length: times }, () => value);

/** Walk a sequence of buffer RMS values through the detector the way the
 *  capture path would, and report which of them counted as speech. Mirrors the
 *  `detect` helper in `src-tauri/src/dictation/capture.rs`, whose cases these
 *  are: the two detectors have to agree buffer for buffer. */
function detect(sequence: number[]): boolean[] {
  let floor = 1;
  return sequence.map((level) => {
    const [speech, next] = track(floor, level);
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

  it("holds a session open while the speech keeps coming", () => {
    const m = monitor();
    m.hear(0.001, 20);
    for (let i = 0; i < 20; i++) m.hear(0.05, SILENCE_STOP_MS - 100);
    expect(m.done()).toBe(false);
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
