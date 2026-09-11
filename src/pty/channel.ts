// The buffer-plus-sink pattern every PTY stream in the app needs: raw bytes
// arrive at high frequency from the backend, are appended to a per-key ring
// buffer (replayed when a view mounts), and are forwarded to the live view's
// writer if one is attached.
//
// Kept generic and outside any store on purpose: routing terminal bytes through
// React state would be wasteful, and the same shape serves an agent's PTY, its
// side shell (./buffers) and a provider sign-in alike.

export type OutputHandler = (bytes: Uint8Array) => void;

const DEFAULT_MAX_BUFFER_BYTES = 256 * 1024;

export interface PtyChannel {
  /** Buffer a chunk and forward it to the live sink (if any). */
  push(key: string, chunk: Uint8Array): void;
  /** Everything buffered for `key`, to replay into a freshly mounted view. */
  get(key: string): Uint8Array | undefined;
  /** Forget the replay history for `key`, keeping any sink attached. */
  clear(key: string): void;
  /** Forget the history *and* the sink — `key` is gone for good. */
  drop(key: string): void;
  /** Point `key`'s live output at `handler`, replacing any previous one — a
   *  single sink per key, by design. Returns an unregister fn that only clears
   *  the slot if `handler` is still the one in it. */
  registerSink(key: string, handler: OutputHandler): () => void;
}

/** An independent buffer+sink channel. `maxBytes` caps each key's ring buffer,
 *  trimming the oldest bytes once it overflows so a long-lived session can't
 *  grow without bound. */
export function createPtyChannel(maxBytes: number = DEFAULT_MAX_BUFFER_BYTES): PtyChannel {
  const buffers = new Map<string, Uint8Array>();
  const sinks = new Map<string, OutputHandler>();

  return {
    push(key, chunk) {
      const existing = buffers.get(key);
      let next: Uint8Array;
      if (!existing) {
        next = chunk;
      } else {
        next = new Uint8Array(existing.length + chunk.length);
        next.set(existing, 0);
        next.set(chunk, existing.length);
      }
      if (next.length > maxBytes) next = next.slice(next.length - maxBytes);
      buffers.set(key, next);
      sinks.get(key)?.(chunk);
    },
    get(key) {
      return buffers.get(key);
    },
    clear(key) {
      buffers.delete(key);
    },
    drop(key) {
      buffers.delete(key);
      sinks.delete(key);
    },
    registerSink(key, handler) {
      sinks.set(key, handler);
      return () => {
        if (sinks.get(key) === handler) sinks.delete(key);
      };
    },
  };
}
