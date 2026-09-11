// Per-agent PTY output buffering, kept outside the Zustand store on purpose:
// raw terminal bytes arrive at high frequency and are consumed imperatively by
// xterm-backed views, so routing them through React state would be wasteful.
//
// Two parallel channels — the agent's own PTY and its side shell — each pair a
// ring buffer (replayed when a view mounts for the first time) with an optional
// live sink (the view's writer). Both are plain ./channel instances; what lives
// here is the per-agent lifecycle layered over them.
//
// This module also owns the lifecycle of the live-terminal cache in
// ./terminals, because "an agent's PTY state" is one thing: the ring buffer and
// the terminal rendering it are cleared, reset and released together.

import { createPtyChannel, type OutputHandler } from "./channel";

// ---- Agent PTY ----------------------------------------------------------------
const agentPty = createPtyChannel();

// ---- Side shell PTY -----------------------------------------------------------
const shellPty = createPtyChannel();

// ---- Live terminal cache ------------------------------------------------------

/** The bits of the ./terminals cache this module drives. */
export interface TerminalCacheHooks {
  /** Wipe a cached terminal's screen, keeping it cached and its sink live. */
  reset: (agentId: string) => void;
  /** Dispose a cached terminal for good. */
  evict: (agentId: string) => void;
}

// Installed by ./terminals when it loads, rather than imported from here.
// Importing xterm outside a browser throws (`self is not defined`), and the
// store actions below flow into node-env tests — a registration keeps the
// renderer out of that import graph. Absent until a terminal view has been
// loaded, in which case there is nothing cached to clean up anyway.
let terminalCache: TerminalCacheHooks | undefined;

export function setTerminalCacheHooks(hooks: TerminalCacheHooks) {
  terminalCache = hooks;
}

/** Buffer an agent-output chunk and forward it to the live view sink (if any). */
export function pushAgentOutput(agentId: string, chunk: Uint8Array) {
  agentPty.push(agentId, chunk);
}

export function getOutputBuffer(agentId: string): Uint8Array | undefined {
  return agentPty.get(agentId);
}

/** Drop an agent's replay history because its PTY restarted (view switch,
 *  resume). Any cached terminal is displaying exactly the history being
 *  dropped, so wipe its screen too — otherwise the next mount would re-attach a
 *  dead session's frame instead of the blank one a restart used to produce. */
export function clearOutputBuffer(agentId: string) {
  agentPty.clear(agentId);
  terminalCache?.reset(agentId);
}

/** Release everything held for an agent that is gone (discarded, archived).
 *  Nothing else clears the shell buffer or the terminal cache, so without this
 *  both grow for the life of the app session. */
export function dropAgentPty(agentId: string) {
  agentPty.drop(agentId);
  shellPty.drop(agentId);
  terminalCache?.evict(agentId);
}

/** Point an agent's live output at `handler`, replacing any previous one — a
 *  single sink per agent, by design. The owner is the agent's cached `Terminal`
 *  (see ./terminals), not the React view: it registers when the terminal is
 *  created and unregisters when the terminal is disposed, so an unmounted
 *  native view keeps feeding its cached screen in the background and no second
 *  view can ever be contending for the same slot. */
export function registerOutputSink(agentId: string, handler: OutputHandler): () => void {
  return agentPty.registerSink(agentId, handler);
}

/** Buffer a shell-output chunk and forward it to the live TermPanel sink. */
export function pushShellOutput(agentId: string, chunk: Uint8Array) {
  shellPty.push(agentId, chunk);
}

export function getShellBuffer(agentId: string): Uint8Array | undefined {
  return shellPty.get(agentId);
}

export function registerShellSink(agentId: string, handler: OutputHandler): () => void {
  return shellPty.registerSink(agentId, handler);
}
