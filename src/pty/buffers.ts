// Side-shell PTY output buffering, kept outside the Zustand store on purpose:
// raw terminal bytes arrive at high frequency and are consumed imperatively by
// the xterm-backed TermPanel, so routing them through React state would be
// wasteful.
//
// A ring buffer (replayed when the panel mounts) paired with an optional live
// sink (the panel's writer), as one ./channel instance; what lives here is the
// per-agent lifecycle layered over it.

import type { EnvironmentId } from "@/store/environments";
import { createPtyChannel, type OutputHandler } from "./channel";

// Every buffer is keyed by environment as well as agent. Agent ids are place
// names from a small recycled pool, so two hosts will both have an agent called
// `fuji` (docs/multi-host-plan.md §1.3) and one's scrollback must never be
// replayed into the other's view.
//
// The environment is always the caller's: the host whose listener a chunk
// arrived on, the scope a view is rendered in, the host an action ran against.
const scoped = (envId: EnvironmentId, agentId: string) => `${envId}:${agentId}`;

const shellPty = createPtyChannel();

/** Buffer a shell-output chunk and forward it to the live TermPanel sink. */
export function pushShellOutput(envId: EnvironmentId, agentId: string, chunk: Uint8Array) {
  shellPty.push(scoped(envId, agentId), chunk);
}

export function getShellBuffer(envId: EnvironmentId, agentId: string): Uint8Array | undefined {
  return shellPty.get(scoped(envId, agentId));
}

export function registerShellSink(
  envId: EnvironmentId,
  agentId: string,
  handler: OutputHandler,
): () => void {
  return shellPty.registerSink(scoped(envId, agentId), handler);
}

/** Release the scrollback held for an agent that is gone (discarded, archived,
 *  deleted with its project). Nothing else clears it, so without this it grows
 *  for the life of the app session. */
export function dropShellBuffer(envId: EnvironmentId, agentId: string) {
  shellPty.drop(scoped(envId, agentId));
}
