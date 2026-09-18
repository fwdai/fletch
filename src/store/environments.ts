// The environment dimension: which engines this client knows about, and which
// one the UI is driving. An "environment" is one Fletch engine — the desktop's
// own, or a paired host (docs/multi-host-plan.md §5.1).
//
// The local environment is a fixed client-side id, present from the first render
// in state `connected`, with no async step and no dependency on the host key or
// on remote access being switched on. It never carries a transport: local ops
// and events go straight down Tauri IPC through the `localTransport` singleton,
// exactly as they do today.
//
// Nothing here connects, pairs or switches anything yet — the dialer PR adds the
// writers. What this buys now is that the two lookups which cannot wait for
// React (the transport a call goes down, and the key a PTY chunk is buffered
// under) have one answer, in one place.

import type { Transport } from "@/api/transport";
import type { SliceCreator } from "./types";

/** The desktop's own engine, following T3Code's `PRIMARY_LOCAL_ENVIRONMENT_ID`.
 *  A constant, never a host key: host public keys identify *remote*
 *  environments only. */
export const LOCAL_ENVIRONMENT_ID = "local";

export type EnvironmentId = string;

export interface EnvironmentEntry {
  id: EnvironmentId;
  name: string;
  kind: "local" | "remote";
  connection: "connected" | "connecting" | "disconnected" | "error";
  /** Why the connection is in `error`, for the (later) environment status. */
  error?: string;
  /** Remote only. The local environment's transport is the module-level
   *  `localTransport`, which is what `activeTransport()` falls back to. */
  transport?: Transport;
}

export interface EnvironmentsSlice {
  environments: Record<EnvironmentId, EnvironmentEntry>;
  activeEnvironmentId: EnvironmentId;
}

/** The state every client starts in: one local environment, connected. */
const initialState = (): EnvironmentsSlice => ({
  environments: {
    [LOCAL_ENVIRONMENT_ID]: {
      id: LOCAL_ENVIRONMENT_ID,
      name: "This Mac",
      kind: "local",
      connection: "connected",
    },
  },
  activeEnvironmentId: LOCAL_ENVIRONMENT_ID,
});

export const createEnvironmentsSlice: SliceCreator<EnvironmentsSlice> = () => initialState();

// The store owns this state, but the readers below are called from outside React
// — on every op and on every PTY chunk — and src/api must not import the store
// (every slice imports @/api, so that would close a cycle). So store/index.ts
// hands its getter in here once, the way src/pty/terminals registers its cache
// hooks with src/pty/buffers. Until it does, the answer is the local
// environment, which is also what the store starts with.
let read: () => EnvironmentsSlice = initialState;

export function setEnvironmentsSource(source: () => EnvironmentsSlice) {
  read = source;
}

/** The active environment's id. Part of every key that would otherwise collide
 *  across hosts: agent ids are place names from a ~300-entry pool, so two hosts
 *  will both have an agent called `fuji` (docs/multi-host-plan.md §1.3). */
export function activeEnvironmentId(): EnvironmentId {
  return read().activeEnvironmentId;
}

export function activeEnvironment(): EnvironmentEntry {
  const state = read();
  return (
    state.environments[state.activeEnvironmentId] ??
    initialState().environments[LOCAL_ENVIRONMENT_ID]
  );
}
