import { describe, expect, it } from "vitest";
import type { Capture, CaptureOptions, ChunkSink } from "../src/dictation/capture";
import {
  bytesToBase64,
  concatPcm16,
  floatToPcm16,
  pcm16ToBase64,
  pcm16ToBytes,
} from "../src/dictation/encode";
import { type DictationApi, DictationSession } from "../src/dictation/session";

describe("chunk encoding", () => {
  it("scales and clamps float samples to 16-bit", () => {
    const pcm = floatToPcm16(Float32Array.from([0, 1, -1, 0.5, 2, -2]));
    expect(Array.from(pcm)).toEqual([0, 32767, -32768, 16384, 32767, -32768]);
  });

  it("writes little-endian bytes whatever the platform", () => {
    expect(Array.from(pcm16ToBytes(Int16Array.from([256, -1])))).toEqual([0, 1, 255, 255]);
  });

  it("round-trips through base64, including chunks past one btoa call", () => {
    const pcm = new Int16Array(70_000);
    for (let i = 0; i < pcm.length; i++) pcm[i] = ((i * 7919) % 65536) - 32768;
    const decoded = Uint8Array.from(atob(pcm16ToBase64(pcm)), (c) => c.charCodeAt(0));
    expect(decoded).toEqual(pcm16ToBytes(pcm));
    expect(bytesToBase64(new Uint8Array([0, 1, 0, 255]))).toBe("AAEA/w==");
  });

  it("concatenates in order", () => {
    expect(
      Array.from(concatPcm16([Int16Array.from([1, 2]), new Int16Array(0), Int16Array.from([3])])),
    ).toEqual([1, 2, 3]);
  });
});

/** A host that records every op, and a mic the test drives by hand. */
function harness(opts: { failAudioAt?: number; beginFails?: boolean; autoStop?: boolean } = {}) {
  const ops: { op: string; args: unknown[] }[] = [];
  let audioCalls = 0;
  const api: DictationApi = {
    async dictationBegin() {
      ops.push({ op: "begin", args: [] });
      if (opts.beginFails) throw new Error("Local dictation is off on your Mac.");
      // `autoStop: undefined` is a host too old to report the field.
      return "autoStop" in opts ? { session: "s1", auto_stop: opts.autoStop } : { session: "s1" };
    },
    async dictationAudio(session, rate, pcm) {
      audioCalls += 1;
      ops.push({ op: "audio", args: [session, rate, pcm] });
      if (opts.failAudioAt === audioCalls) throw new Error("frame too large");
      // Out-of-order delivery would show up as a reordered `ops` list.
      await new Promise((r) => setTimeout(r, audioCalls % 2 ? 5 : 0));
      return null;
    },
    async dictationEnd(session) {
      ops.push({ op: "end", args: [session] });
      return { text: "hello world" };
    },
    async dictationCancel(session) {
      ops.push({ op: "cancel", args: [session] });
      return null;
    },
  };
  let sink: ChunkSink | null = null;
  let stopped = 0;
  let doneTalking: (() => void) | undefined;
  const mic = {
    started: 0,
    /** Feed a chunk as the worklet would. */
    speak(samples: number[]) {
      sink?.(Int16Array.from(samples), 48_000);
    },
    /** Report the pause, as the capture's silence monitor would. */
    pause() {
      doneTalking?.();
    },
    /** Whether `capture.ts` would have armed its silence watcher at all: it
     *  no-ops without a callback, which is the auto-stop opt-out. */
    get armed() {
      return doneTalking !== undefined;
    },
    get stopped() {
      return stopped;
    },
  };
  const startCapture = async (onChunk: ChunkSink, opts: CaptureOptions): Promise<Capture> => {
    mic.started += 1;
    sink = onChunk;
    doneTalking = opts.onDoneTalking;
    return {
      rate: 48_000,
      async stop() {
        stopped += 1;
        // The real capture flushes its tail into the sink before resolving.
        sink?.(Int16Array.from([9]), 48_000);
        sink = null;
      },
    };
  };
  return { ops, api, mic, startCapture };
}

describe("DictationSession", () => {
  it("opens the host session before the mic, streams chunks in order, and ends with the text", async () => {
    const h = harness();
    const s = new DictationSession(h.api, h.startCapture);
    await s.start();
    expect(h.ops.map((o) => o.op)).toEqual(["begin"]);
    expect(h.mic.started).toBe(1);

    h.mic.speak([1, 2]);
    h.mic.speak([3]);
    const text = await s.stop();

    expect(text).toBe("hello world");
    expect(h.mic.stopped).toBe(1);
    expect(h.ops.map((o) => o.op)).toEqual(["begin", "audio", "audio", "audio", "end"]);
    // Every chunk names the session and the capture rate, and the tail the
    // mic flushed at stop went out before `end`.
    const audio = h.ops.filter((o) => o.op === "audio");
    expect(audio.map((o) => o.args[0])).toEqual(["s1", "s1", "s1"]);
    expect(audio.map((o) => o.args[1])).toEqual([48_000, 48_000, 48_000]);
    expect(audio.map((o) => o.args[2])).toEqual([
      pcm16ToBase64(Int16Array.from([1, 2])),
      pcm16ToBase64(Int16Array.from([3])),
      pcm16ToBase64(Int16Array.from([9])),
    ]);
  });

  it("a mic that will not open cancels the host session", async () => {
    const h = harness();
    const s = new DictationSession(h.api, async () => {
      throw new Error("Microphone access was denied.");
    });
    await expect(s.start()).rejects.toThrow("denied");
    expect(h.ops.map((o) => o.op)).toEqual(["begin", "cancel"]);
  });

  it("a host that cannot transcribe fails before the mic opens", async () => {
    const h = harness({ beginFails: true });
    const s = new DictationSession(h.api, h.startCapture);
    await expect(s.start()).rejects.toThrow("off on your Mac");
    expect(h.mic.started).toBe(0);
  });

  it("a chunk that fails to send fails the stop instead of transcribing a hole", async () => {
    const h = harness({ failAudioAt: 1 });
    const s = new DictationSession(h.api, h.startCapture);
    await s.start();
    h.mic.speak([1]);
    h.mic.speak([2]);
    await expect(s.stop()).rejects.toThrow("frame too large");
    // Later chunks were dropped rather than sent past the failure, the host
    // was told to drop the session, and no transcription was asked for.
    expect(h.ops.filter((o) => o.op === "audio")).toHaveLength(1);
    expect(h.ops.map((o) => o.op)).toEqual(["begin", "audio", "cancel"]);
    expect(h.mic.stopped).toBe(1);
  });

  it("a mic that reports the pause ends the session through the normal stop", async () => {
    const h = harness();
    const s = new DictationSession(h.api, h.startCapture);
    // What `useDictation` does with the signal: run the very stop a tap runs.
    const stops: Promise<string>[] = [];
    await s.start(() => stops.push(s.stop()));

    h.mic.speak([1, 2]);
    h.mic.pause();

    expect(await Promise.all(stops)).toEqual(["hello world"]);
    expect(h.ops.map((o) => o.op)).toEqual(["begin", "audio", "audio", "end"]);
    expect(h.mic.stopped).toBe(1);
  });

  it("a pause that lands while the user is stopping cannot end the session twice", async () => {
    const h = harness();
    const s = new DictationSession(h.api, h.startCapture);
    const stops: Promise<string>[] = [];
    await s.start(() => stops.push(s.stop()));
    h.mic.speak([1]);

    // The tap wins, and the pause arriving inside it is dropped rather than
    // stopping a mic that is already closing or transcribing twice.
    const tapped = s.stop();
    h.mic.pause();

    expect(await tapped).toBe("hello world");
    expect(stops).toEqual([]);
    expect(h.ops.filter((o) => o.op === "end")).toHaveLength(1);
    expect(h.mic.stopped).toBe(1);
  });

  /** The Mac owns "Stop after a pause" and answers it on `begin`, per session;
   *  the phone is the only side that can act on it. Started here exactly as
   *  `useDictation` starts one — which now means always offering the callback
   *  and letting the host's answer decide — so what is under test is the wiring
   *  from that answer to the mic, not a restatement of it. */
  async function startAsTheHookWould(opts: { autoStop?: boolean } | Record<string, never>) {
    const h = harness(opts);
    const s = new DictationSession(h.api, h.startCapture);
    const stops: Promise<string>[] = [];
    await s.start(() => stops.push(s.stop()));
    return { h, s, stops };
  }

  it("a session begun with auto_stop: false never arms the silence watcher", async () => {
    const { h, s, stops } = await startAsTheHookWould({ autoStop: false });
    expect(h.mic.armed).toBe(false);

    h.mic.speak([1]);
    // Nothing the mic could ever report ends this session: without the callback
    // `watchForSilence` never starts a timer, so the pause and the never-spoke
    // deadline alike go unasked — the desktop's `should_auto_stop(false, …)`.
    h.mic.pause();
    expect(stops).toEqual([]);
    expect(h.mic.stopped).toBe(0);

    // A tap still ends it, which is the only way left.
    expect(await s.stop()).toBe("hello world");
    expect(h.mic.stopped).toBe(1);
  });

  it("a session begun with auto_stop: true stops itself, and so does one whose host never said", async () => {
    for (const opts of [{ autoStop: true }, {}]) {
      const { h, stops } = await startAsTheHookWould(opts);
      expect(h.mic.armed).toBe(true);
      h.mic.speak([1]);
      h.mic.pause();
      expect(await Promise.all(stops)).toEqual(["hello world"]);
      expect(h.ops.map((o) => o.op)).toEqual(["begin", "audio", "audio", "end"]);
    }
  });

  it("cancel releases the mic and the host session, and is idempotent", async () => {
    const h = harness();
    const s = new DictationSession(h.api, h.startCapture);
    const stops: Promise<string>[] = [];
    await s.start(() => stops.push(s.stop()));
    await s.cancel();
    await s.cancel();
    // A pause reported after the teardown — the screen was left mid-session —
    // has nothing left to end.
    h.mic.pause();
    expect(stops).toEqual([]);
    expect(h.mic.stopped).toBe(1);
    expect(h.ops.map((o) => o.op)).toEqual(["begin", "cancel"]);
    expect(await s.stop()).toBe("");
  });
});
