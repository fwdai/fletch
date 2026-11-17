// The environment dimension: which engines this client knows about, and which
// one is on screen (the host of what the main pane shows). An "environment" is
// one Fletch engine — the desktop's own, or a paired host
// (docs/multi-host-plan.md §5.1).
//
// The local environment is a fixed client-side id, present from the first render
// in state `connected`, with no async step and no dependency on the host key or
// on remote access being switched on. It never carries a transport: local ops
// and events go straight down Tauri IPC through the `localTransport` singleton,
// exactly as they do today.
//
// The writers below are what the paired-host lifecycle (src/remote/hosts.ts)
// drives an entry with. A remote entry comes with its host's partition of the
// store (`hostStates`, see ./hostState): published together, dropped together.
//
// Which environment is on screen (`switchEnvironment`) and what a handshake
// re-reads live in ./environmentSwitch, which needs @/api and so cannot live
// here (see the note on `setEnvironmentsSource` at the bottom).

import type { Transport } from "@/api/transport";
import type { HostProtocol, HostProvider } from "@/remote/types";
import { BLANK_HOST_STATE, type HostState } from "./hostState";
import type { SliceCreator } from "./types";

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
   *  this failure, so a host's chip says "reconnecting" rather than "offline".
   *  False with a non-`connected` state is a failure only the user can clear
   *  (unpaired, a changed host key). */
  retrying?: boolean;
  /** Remote only. The local environment's transport is the module-level
   *  `localTransport`. */
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
}

export interface EnvironmentsSlice {
  environments: Record<EnvironmentId, EnvironmentEntry>;
  /** The host on screen: the host of what the main pane shows. Every
   *  selection that belongs to a host puts it there (`onHost`); nothing asks
   *  the user to pick it. */
  activeEnvironmentId: EnvironmentId;
  /** Each paired host's partition of the store, by environment id: its
   *  workspace and every map keyed by something that belongs to its engine
   *  (./hostState). The local environment has none — its state is the store's
   *  own top-level keys. Read through a host's scope (./hostScope), never
   *  directly. */
  hostStates: Record<EnvironmentId, HostState>;

  /** Add a remote environment, or merge `entry` into the one already under its
   *  id — a reconnect re-reports the same host with a fresh descriptor and the
   *  entry it lands on must not lose anything the caller left out. A host seen
   *  for the first time gets a blank partition with its entry. */
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
  /** Forget a paired host, and its partition with it. The local environment is
   *  not removable: it is this process's own engine, present from the first
   *  render, and nothing in the app has a fallback for its absence. */
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
  hostStates: {},

  upsertEnvironment: (entry) =>
    set((s) => ({
      environments: { ...s.environments, [entry.id]: { ...s.environments[entry.id], ...entry } },
      ...(entry.id === LOCAL_ENVIRONMENT_ID || s.hostStates[entry.id]
        ? {}
        : { hostStates: { ...s.hostStates, [entry.id]: BLANK_HOST_STATE } }),
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
      const { [id]: _partition, ...hostStates } = s.hostStates;
      return {
        environments: rest,
        hostStates,
        // A removal must not strand the active id on an entry that is no longer
        // there: forgetting the host on screen puts This Mac on screen, whose
        // state never left the store.
        activeEnvironmentId:
          s.activeEnvironmentId === id ? LOCAL_ENVIRONMENT_ID : s.activeEnvironmentId,
      };
    }),
});

// The store owns this state, but the readers below are called from outside React
// — on every op and on every PTY chunk — and src/api must not import the store
// (the store is built over @/api's engine, so that would close a cycle). So store/index.ts
// hands its getter in here once, the way src/pty/terminals registers its cache
// hooks with src/pty/buffers. Until it does, the answer is the local
// environment, which is also what the store starts with.
let read: () => EnvironmentsState = initialState;

export function setEnvironmentsSource(source: () => EnvironmentsState) {
  read = source;
}

/** The id of the host on screen: the one the main pane is scoped to. */
export function activeEnvironmentId(): EnvironmentId {
  return read().activeEnvironmentId;
}

/** The entry for `id`, falling back to the local one for an id that is not
 *  there — a host forgotten while something about it was still in flight. */
export function environmentEntry(id: EnvironmentId): EnvironmentEntry {
  return read().environments[id] ?? initialState().environments[LOCAL_ENVIRONMENT_ID];
}

export function activeEnvironment(): EnvironmentEntry {
  return environmentEntry(activeEnvironmentId());
}

/** Every environment's id, This Mac first (it is always there) and then the
 *  paired hosts by name: the order of every surface that lists them all, so
 *  none reshuffles as connections come and go. */
export function hostOrder(environments: Record<EnvironmentId, EnvironmentEntry>): EnvironmentId[] {
  return Object.values(environments)
    .sort((a, b) =>
      a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "local" ? -1 : 1,
    )
    .map((e) => e.id);
}
