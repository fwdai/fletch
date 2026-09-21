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
// The writers below are what the paired-host lifecycle (src/remote/hosts.ts)
// drives an entry with, and what this buys is that the two lookups which cannot
// wait for React (the transport a call goes down, and the key a PTY chunk is
// buffered under) have one answer, in one place.
//
// The switch itself — parking one environment's view and restoring another's —
// lives in ./environmentSwitch, which needs @/api and so cannot live here (see
// the note on `setEnvironmentsSource` at the bottom).

import type { Transport } from "@/api/transport";
import type { HostProtocol, HostProvider } from "@/remote/types";
import type { AppState, SliceCreator } from "./types";

/** The desktop's own engine, following T3Code's `PRIMARY_LOCAL_ENVIRONMENT_ID`.
 *  A constant, never a host key: host public keys identify *remote*
 *  environments only. */
export const LOCAL_ENVIRONMENT_ID = "local";

export type EnvironmentId = string;

export type ConnectionStatus = "connected" | "connecting" | "disconnected" | "error";

export interface EnvironmentEntry {
  id: EnvironmentId;
  name: string;
  kind: "local" | "remote";
  connection: ConnectionStatus;
  /** Why the connection is in `error`, for the (later) environment status. */
  error?: string;
  /** Remote only. Mirrors `RemoteClient.retrying`: a retry is scheduled behind
   *  this failure, so the switcher says "reconnecting" rather than "offline".
   *  False with a non-`connected` state is a failure only the user can clear
   *  (unpaired, remote access off, a changed host key). */
  retrying?: boolean;
  /** Remote only. The local environment's transport is the module-level
   *  `localTransport`, which is what `activeTransport()` falls back to. */
  transport?: Transport;
  /** Remote only: the app version the host reported in `hello`/`pair`
   *  (`HostInfo.appVersion`), for the rows that identify it. In memory only —
   *  it is whatever the last handshake said, so there is nothing worth keeping
   *  on disk. Never gated on: capability is `protocol.ops` membership
   *  (src/remote/types.ts). */
  appVersion?: string;
  /** Remote only: what the connected host said it answers, from the last
   *  handshake. Absent for a host that sent none (one from before the field) or
   *  one that has not been greeted yet. The local environment is never gated
   *  (docs/multi-host-plan.md §5.1), so it never has one. */
  protocol?: HostProtocol;
  /** Remote only: which provider CLIs the host has and which of them are signed
   *  in, from `host_providers` on the last handshake. Absent for a host that
   *  does not answer the op, and for one that has not been greeted yet — in
   *  both cases the client knows nothing and blocks nothing (see
   *  `providerReason` in ./capabilities). In memory only, like `protocol`. */
  providers?: HostProvider[];
  /** What this environment was showing when the user last left it — the
   *  workspace snapshot and every map keyed by something that belongs to one
   *  engine (agent ids, checkouts, repo paths). Written and read only by
   *  `switchEnvironment` (./environmentSwitch), which owns the list; absent
   *  until the user has switched away from this environment once. */
  stash?: Partial<AppState>;
  /** The agent selected here when the user last left, restored on the way
   *  back. Kept out of `stash` because it is the one piece of the parked view
   *  the switcher itself may want to read. */
  lastSelectedAgentId?: string | null;
}

export interface EnvironmentsSlice {
  environments: Record<EnvironmentId, EnvironmentEntry>;
  activeEnvironmentId: EnvironmentId;

  /** Add a remote environment, or merge `entry` into the one already under its
   *  id — a reconnect re-reports the same host with a fresh descriptor and the
   *  entry it lands on must not lose anything the caller left out. */
  upsertEnvironment: (entry: EnvironmentEntry) => void;
  /** Mirror one connection's state onto its entry. `error` and `retrying` are
   *  written as given, so a state change with no reason clears the last one. */
  setEnvironmentConnection: (
    id: EnvironmentId,
    connection: ConnectionStatus,
    error?: string,
    retrying?: boolean,
  ) => void;
  /** Record what a host answered `host_providers` with. Its own writer rather
   *  than a field on `upsertEnvironment` because the answer arrives after the
   *  handshake that published the entry, and must not carry a stale connection
   *  state back with it. */
  setEnvironmentProviders: (id: EnvironmentId, providers: HostProvider[]) => void;
  /** Forget a paired host. The local environment is not removable: it is this
   *  process's own engine, present from the first render, and nothing in the
   *  app has a fallback for its absence. */
  removeEnvironment: (id: EnvironmentId) => void;
}

type EnvironmentsState = Pick<EnvironmentsSlice, "environments" | "activeEnvironmentId">;

/** The state every client starts in: one local environment, connected. */
const initialState = (): EnvironmentsState => ({
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

export const createEnvironmentsSlice: SliceCreator<EnvironmentsSlice> = (set) => ({
  ...initialState(),

  upsertEnvironment: (entry) =>
    set((s) => ({
      environments: { ...s.environments, [entry.id]: { ...s.environments[entry.id], ...entry } },
    })),

  setEnvironmentConnection: (id, connection, error, retrying) =>
    set((s) => {
      const entry = s.environments[id];
      // A state arriving for a host that has since been forgotten is not worth
      // reviving the entry for.
      if (!entry) return {};
      return {
        environments: { ...s.environments, [id]: { ...entry, connection, error, retrying } },
      };
    }),

  setEnvironmentProviders: (id, providers) =>
    set((s) => {
      const entry = s.environments[id];
      // Same rule as `setEnvironmentConnection`: an answer for a host that has
      // since been forgotten is not worth reviving the entry for.
      if (!entry) return {};
      return { environments: { ...s.environments, [id]: { ...entry, providers } } };
    }),

  removeEnvironment: (id) =>
    set((s) => {
      if (id === LOCAL_ENVIRONMENT_ID) return {};
      const { [id]: _gone, ...rest } = s.environments;
      return {
        environments: rest,
        // A removal must not strand the active id on an entry that is no longer
        // there. This is the backstop, not the path: forgetting the host on
        // screen goes through `switchEnvironment` first (see remote/hosts.ts),
        // which is what actually puts This Mac's view back.
        activeEnvironmentId:
          s.activeEnvironmentId === id ? LOCAL_ENVIRONMENT_ID : s.activeEnvironmentId,
      };
    }),
});

// The store owns this state, but the readers below are called from outside React
// — on every op and on every PTY chunk — and src/api must not import the store
// (every slice imports @/api, so that would close a cycle). So store/index.ts
// hands its getter in here once, the way src/pty/terminals registers its cache
// hooks with src/pty/buffers. Until it does, the answer is the local
// environment, which is also what the store starts with.
let read: () => EnvironmentsState = initialState;

export function setEnvironmentsSource(source: () => EnvironmentsState) {
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
