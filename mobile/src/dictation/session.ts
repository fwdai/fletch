// One dictation, from the tap that opens the mic to the transcript: opens a
// host session, streams every chunk in order while the user talks, and on stop
// waits for the backlog before asking the host to transcribe. No React and no
// browser APIs — the capture is injected — so the sequencing is unit-tested.

import type { Api } from "../api";
import type { Capture, CaptureOptions, ChunkSink } from "./capture";
import { pcm16ToBase64 } from "./encode";

export type DictationApi = Pick<
  Api,
  "dictationBegin" | "dictationAudio" | "dictationEnd" | "dictationCancel"
>;

export type CaptureStarter = (onChunk: ChunkSink, opts: CaptureOptions) => Promise<Capture>;

export class DictationSession {
  private session: string | null = null;
  private capture: Capture | null = null;
  /** Chunks go out one at a time, in order: the host fixes the sample rate on
   *  the first and appends the rest, and one request in flight keeps a slow
   *  link from piling up against the connection's in-flight cap. */
  private queue: Promise<void> = Promise.resolve();
  /** The first chunk that failed to send. Later chunks are dropped — the
   *  transcript would have a hole in it either way — and `stop` reports it
   *  instead of asking the host to transcribe half a sentence. */
  private failed: Error | null = null;
  private done = false;

  constructor(
    private readonly api: DictationApi,
    private readonly startCapture: CaptureStarter,
  ) {}

  /** Open the host session, then the mic. The host is asked first so a Mac
   *  that can't transcribe (engine off, model missing) answers before the
   *  user has said anything.
   *
   *  `onAutoStop` fires when the mic hears the pause that ends a session. It is
   *  the caller's job to run the same `stop` a tap on the button runs, so that
   *  hands-free and by-hand take one code path. */
  async start(onAutoStop?: () => void): Promise<void> {
    const { session } = await this.api.dictationBegin();
    this.session = session;
    // A pause that lands while the session is already ending — the user tapped
    // stop at the same moment — is nobody's to act on: `done` says so.
    const onDoneTalking = onAutoStop
      ? () => {
          if (!this.done) onAutoStop();
        }
      : undefined;
    try {
      this.capture = await this.startCapture((pcm, rate) => this.send(pcm, rate), {
        onDoneTalking,
      });
    } catch (e) {
      await this.cancel();
      throw e;
    }
  }

  /** Close the mic, drain the backlog, transcribe. Resolves with the
   *  transcript — empty when the host heard no speech. */
  async stop(): Promise<string> {
    const session = this.session;
    if (!session || this.done) return "";
    this.done = true;
    // `stop` flushes the tail into `send` synchronously before resolving, so
    // awaiting the queue afterwards covers the last chunk too.
    await this.capture?.stop();
    this.capture = null;
    await this.queue;
    if (this.failed) {
      void this.api.dictationCancel(session).catch(() => {});
      this.session = null;
      throw this.failed;
    }
    this.session = null;
    const { text } = await this.api.dictationEnd(session);
    return text;
  }

  /** Throw the audio away, on both ends. Safe to call at any point, including
   *  after `stop`, when there is nothing left to cancel. */
  async cancel(): Promise<void> {
    const session = this.session;
    this.session = null;
    this.done = true;
    const capture = this.capture;
    this.capture = null;
    await capture?.stop().catch(() => {});
    if (session) await this.api.dictationCancel(session).catch(() => {});
  }

  private send(pcm: Int16Array, rate: number) {
    const session = this.session;
    if (!session || this.failed) return;
    const encoded = pcm16ToBase64(pcm);
    this.queue = this.queue.then(async () => {
      if (this.failed) return;
      try {
        await this.api.dictationAudio(session, rate, encoded);
      } catch (e) {
        this.failed = e instanceof Error ? e : new Error(String(e));
      }
    });
  }
}
