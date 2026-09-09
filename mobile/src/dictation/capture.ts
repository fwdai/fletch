// Microphone capture in the webview: `getUserMedia` into an `AudioWorklet`
// tap, batched into ~1 s chunks of 16-bit PCM. Nothing here knows about the
// host; `session.ts` decides where the chunks go.

import { concatPcm16, floatToPcm16 } from "./encode";

/** How much audio one chunk holds. A second keeps the wire under the relay's
 *  message-rate limit with room to spare and the host's per-chunk cap far off. */
const CHUNK_MS = 1000;

export type ChunkSink = (pcm: Int16Array, rate: number) => void;

export interface Capture {
  /** The audio session's rate — typically 48 000 on an iPhone. The host
   *  resamples, so nothing here does. */
  rate: number;
  /** Release the mic. Flushes what was buffered into the sink first. */
  stop(): Promise<void>;
}

/** Whether this webview has the APIs at all. False in the vitest jsdom and in
 *  a WKWebView with no mic entitlement, where the button is best not shown. */
export function canCapture(): boolean {
  return (
    typeof navigator !== "undefined" &&
    !!navigator.mediaDevices?.getUserMedia &&
    typeof AudioContext !== "undefined"
  );
}

/** Open the mic and start delivering chunks. Rejects with a message the user
 *  can act on when the OS says no.
 *
 *  Call this from the tap handler: WebKit only lets an `AudioContext` start
 *  inside a user gesture, and the context is created before the first `await`
 *  so the gesture still covers it. */
export async function startCapture(onChunk: ChunkSink): Promise<Capture> {
  const ctx = new AudioContext();
  let stream: MediaStream;
  try {
    await ctx.resume();
    stream = await navigator.mediaDevices.getUserMedia({
      audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true },
    });
  } catch (e) {
    await ctx.close().catch(() => {});
    throw new Error(describe(e));
  }

  const rate = ctx.sampleRate;
  const framesPerChunk = Math.round((rate * CHUNK_MS) / 1000);
  let pending: Float32Array[] = [];
  let pendingFrames = 0;

  const flush = () => {
    if (pendingFrames === 0) return;
    const chunk = concatPcm16(pending.map(floatToPcm16));
    pending = [];
    pendingFrames = 0;
    onChunk(chunk, rate);
  };
  const push = (frames: Float32Array) => {
    pending.push(frames);
    pendingFrames += frames.length;
    if (pendingFrames >= framesPerChunk) flush();
  };

  const source = ctx.createMediaStreamSource(stream);
  let teardown: () => void;
  try {
    // The worklet is the right tool: it runs off the main thread and costs a
    // copy per render quantum. `ScriptProcessorNode` is the deprecated
    // fallback — for a WebKit build without worklets, or a module the page's
    // CSP refused to load.
    teardown = await tapWithWorklet(ctx, source, push).catch(() =>
      tapWithScriptProcessor(ctx, source, push),
    );
  } catch (e) {
    for (const t of stream.getTracks()) t.stop();
    await ctx.close().catch(() => {});
    throw new Error(`Couldn't start the microphone: ${describe(e)}`);
  }

  return {
    rate,
    async stop() {
      teardown();
      for (const t of stream.getTracks()) t.stop();
      await ctx.close().catch(() => {});
      flush();
    },
  };
}

type Push = (frames: Float32Array) => void;

/** Resolves to the teardown once the worklet is wired. The module is emitted
 *  as its own asset (see `assetsInlineLimit` in vite.config.ts): inlined as a
 *  `data:` URL it would be a script the CSP does not allow. */
async function tapWithWorklet(
  ctx: AudioContext,
  source: MediaStreamAudioSourceNode,
  push: Push,
): Promise<() => void> {
  if (typeof AudioWorkletNode === "undefined" || !ctx.audioWorklet) {
    throw new Error("no AudioWorklet");
  }
  await ctx.audioWorklet.addModule(new URL("./worklet.js", import.meta.url));
  const tap = new AudioWorkletNode(ctx, "fletch-pcm-tap", {
    numberOfInputs: 1,
    numberOfOutputs: 0,
  });
  tap.port.onmessage = (e: MessageEvent<Float32Array>) => push(e.data);
  source.connect(tap);
  return () => {
    tap.port.onmessage = null;
    source.disconnect();
  };
}

function tapWithScriptProcessor(
  ctx: AudioContext,
  source: MediaStreamAudioSourceNode,
  push: Push,
): () => void {
  const tap = ctx.createScriptProcessor(4096, 1, 1);
  tap.onaudioprocess = (e) => push(Float32Array.from(e.inputBuffer.getChannelData(0)));
  source.connect(tap);
  // A ScriptProcessorNode only runs while connected to the destination.
  tap.connect(ctx.destination);
  return () => {
    tap.onaudioprocess = null;
    source.disconnect();
    tap.disconnect();
  };
}

/** `getUserMedia`'s DOMExceptions, in words. A denied grant can only be undone
 *  in iOS Settings, so say so rather than fail silently. */
function describe(e: unknown): string {
  const name = (e as { name?: string })?.name;
  if (name === "NotAllowedError" || name === "SecurityError") {
    return "Microphone access was denied. Allow it for Fletch in iOS Settings › Privacy & Security.";
  }
  if (name === "NotFoundError") return "No microphone was found on this device.";
  if (name === "NotReadableError") return "The microphone is in use by another app.";
  return e instanceof Error ? e.message : String(e);
}
