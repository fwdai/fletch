// Per-agent PTY output buffering, kept outside the Zustand store on purpose:
// raw terminal bytes arrive at high frequency and are consumed imperatively by
// xterm-backed views, so routing them through React state would be wasteful.
//
// Two parallel channels — the agent's own PTY and its side shell — each pair a
// ring buffer (replayed when a view remounts after a tab/view switch) with an
// optional live sink (the mounted view's writer).

export type OutputHandler = (bytes: Uint8Array) => void;

const MAX_BUFFER_BYTES = 256 * 1024;

// ---- Agent PTY ----------------------------------------------------------------
const outputSinks = new Map<string, OutputHandler>();
const outputBuffers = new Map<string, Uint8Array>();

// ---- Side shell PTY -----------------------------------------------------------
const shellSinks = new Map<string, OutputHandler>();
const shellBuffers = new Map<string, Uint8Array>();

/** Append a chunk to an agent's ring buffer, trimming the oldest bytes once it
 *  grows past the cap so a long-lived session can't grow without bound. */
function appendToRing(
  buffers: Map<string, Uint8Array>,
  agentId: string,
  chunk: Uint8Array,
) {
  const existing = buffers.get(agentId);
  let next: Uint8Array;
  if (!existing) {
    next = chunk;
  } else {
    next = new Uint8Array(existing.length + chunk.length);
    next.set(existing, 0);
    next.set(chunk, existing.length);
  }
  if (next.length > MAX_BUFFER_BYTES) {
    next = next.slice(next.length - MAX_BUFFER_BYTES);
  }
  buffers.set(agentId, next);
}

/** Buffer an agent-output chunk and forward it to the live view sink (if any). */
export function pushAgentOutput(agentId: string, chunk: Uint8Array) {
  appendToRing(outputBuffers, agentId, chunk);
  outputSinks.get(agentId)?.(chunk);
}

export function getOutputBuffer(agentId: string): Uint8Array | undefined {
  return outputBuffers.get(agentId);
}

export function clearOutputBuffer(agentId: string) {
  outputBuffers.delete(agentId);
}

export function registerOutputSink(
  agentId: string,
  handler: OutputHandler,
): () => void {
  outputSinks.set(agentId, handler);
  return () => {
    if (outputSinks.get(agentId) === handler) outputSinks.delete(agentId);
  };
}

/** Buffer a shell-output chunk and forward it to the live TermPanel sink. */
export function pushShellOutput(agentId: string, chunk: Uint8Array) {
  appendToRing(shellBuffers, agentId, chunk);
  shellSinks.get(agentId)?.(chunk);
}

export function getShellBuffer(agentId: string): Uint8Array | undefined {
  return shellBuffers.get(agentId);
}

export function registerShellSink(
  agentId: string,
  handler: OutputHandler,
): () => void {
  shellSinks.set(agentId, handler);
  return () => {
    if (shellSinks.get(agentId) === handler) shellSinks.delete(agentId);
  };
}
