// One connection per paired host, and the mapping from a client's life onto the
// environment entry the UI reads.
//
// The shape is the phone's, because the phone already solves it: one
// `ProtocolClient` per host, dialled LAN-then-relay through `candidatesFor`,
// reconnecting on its own backoff. There is no retry loop here — a second one
// would race the client's. What this adds is the *plural*: a map keyed by host
// public key, which is also the environment id, so two hosts cannot tread on
// each other's state (agent names collide across hosts,
// docs/multi-host-plan.md §1.3).
//
// Nothing here touches the local engine, and an empty registry does nothing at
// all: with no saved hosts there is no client, no socket and no timer.

import { RemoteTransport } from "@/api/transport";
import type { ConnectionStatus, EnvironmentsSlice } from "@/store/environments";
import { parseAddress } from "./pairing";
import type { ConnectionState, DeviceInfo, HelloResult, HostTarget, RemoteClient } from "./types";

/** The three store writers the registry drives. Taken as an interface rather
 *  than reached for through the app store, so the lifecycle is testable with
 *  nothing but a fake client and a fake set of writers. */
export type EnvironmentWriters = Pick<
  EnvironmentsSlice,
  "upsertEnvironment" | "setEnvironmentConnection" | "removeEnvironment"
>;

/** Where to dial one host under management, and what to call it. Mirrors the
 *  persisted `SavedHost` minus `pairedAt`: the registry neither reads nor
 *  writes the settings row. */
export interface HostRecord {
  hostKey: string;
  name: string;
  /** `<ip-or-name>[:port]`. */
  addr: string;
  relay?: string;
}

/** A parsed `fletch://pair` link good enough to pair with. The desktop insists
 *  on the host key where the phone may pin one on first contact: here the key
 *  *is* the environment id, so there is nowhere to put a host without one. */
export interface PairTarget extends HostTarget {
  hostKey: string;
  pairingToken: string;
}

export interface HostRegistryOptions {
  writers: EnvironmentWriters;
  /** What this desktop tells a host about itself. A thunk because the app
   *  version is an async read and nothing needs it until a client is built. */
  device: () => Promise<DeviceInfo>;
  /** Builds the client for one host. Injected so the wiring owns the secure
   *  transport and a test can hand in a fake. */
  newClient: (device: DeviceInfo) => RemoteClient;
}

/** The client's five states as the three an entry has. `pairing` is a
 *  connection being established like any other as far as a status dot goes;
 *  the pane tells them apart by the words beside it. */
const asEntryConnection = (state: ConnectionState): ConnectionStatus =>
  state === "pairing" ? "connecting" : state;

export interface HostRegistry {
  /** Register `record` as an environment and start dialling it — it resolves
   *  once the entry is published and the dial is under way, not once the host
   *  has answered. Idempotent: a host already under management is left
   *  connecting on its own backoff. */
  adopt(record: HostRecord): Promise<void>;
  /** Pair from a parsed link, and answer with the record to save. On failure
   *  the entry and its client are dropped — nothing was saved, so nothing
   *  should be left behind — and the reason is rethrown for the pane. */
  pair(target: PairTarget): Promise<HostRecord>;
  /** Stop for good, and drop the environment. */
  forget(hostKey: string): void;
  /** The live client for a host, for whatever needs to talk to it directly. */
  get(hostKey: string): RemoteClient | undefined;
}

export function createHostRegistry(opts: HostRegistryOptions): HostRegistry {
  const clients = new Map<string, RemoteClient>();
  const { writers } = opts;

  /** Build the client, publish the entry, start the dial, and subscribe the two
   *  things the entry is made of: the connection state, and the descriptor
   *  every handshake carries.
   *
   *  The order is load-bearing. `onState` replays the client's current state on
   *  subscribe, so the dial is started first and subscribed in the same
   *  synchronous turn: nothing can run in between, and the replay opens with
   *  the `connecting` the dial just set rather than the `disconnected` the
   *  client was built in. The snapshot subscription is still in place long
   *  before the handshake can answer, which takes at least one socket open. */
  async function start(
    record: HostRecord,
    target: HostTarget,
  ): Promise<{ client: RemoteClient; dial: Promise<HelloResult> }> {
    const client = opts.newClient(await opts.device());
    clients.set(record.hostKey, client);
    let name = record.name;
    // Published before the dial, in `connecting`: a host that is offline has to
    // show up in the pane as a host that is offline, not as nothing at all.
    writers.upsertEnvironment({
      id: record.hostKey,
      name,
      kind: "remote",
      connection: "connecting",
      transport: new RemoteTransport(client),
    });
    const dial = client.connect(target);
    // `4003` (this device is no longer paired) and `4004` (remote access off)
    // arrive here like any other failure — `error` with the reason
    // `CLOSE_REASONS` gives, the same words the phone shows. What differs is
    // that the client schedules no retry behind them, which is what
    // `client.retrying` tells the pane.
    client.onState((state, error) =>
      writers.setEnvironmentConnection(record.hostKey, asEntryConnection(state), error),
    );
    client.onSnapshot((snapshot) => {
      // The host's own name wins over the one the pairing link carried, and the
      // descriptor is what 12b gates a remote environment's UI on.
      name = snapshot.host.name || name;
      writers.upsertEnvironment({
        id: record.hostKey,
        name,
        kind: "remote",
        connection: "connected",
        protocol: snapshot.protocol,
      });
    });
    return { client, dial };
  }

  function forget(hostKey: string): void {
    // `disconnect`, not just a close: it forgets the target too, so nothing can
    // later greet a host this device is no longer paired with.
    clients.get(hostKey)?.disconnect();
    clients.delete(hostKey);
    writers.removeEnvironment(hostKey);
  }

  return {
    async adopt(record) {
      if (clients.has(record.hostKey)) return;
      const address = parseAddress(record.addr);
      if (!address) {
        // Nothing to dial, so no client is built — but the host is still
        // listed, with the one thing the user can act on.
        writers.upsertEnvironment({
          id: record.hostKey,
          name: record.name,
          kind: "remote",
          connection: "error",
          error: `Saved address "${record.addr}" cannot be dialled. Pair this host again.`,
        });
        return;
      }
      const { dial } = await start(record, {
        ...address,
        hostKey: record.hostKey,
        name: record.name,
        relay: record.relay,
      });
      // Started, not waited on: the entry is already published and the client
      // reports its own progress and failures through `onState`, retrying on
      // its own backoff. A host that is asleep must not hold up the host after
      // it, nor anything that called this.
      void dial.catch(() => {});
    },

    async pair(target) {
      // Re-pairing a host already under management replaces it: the old client
      // holds a device registration this handshake is about to supersede.
      forget(target.hostKey);
      const record: HostRecord = {
        hostKey: target.hostKey,
        name: target.name || target.host,
        addr: `${target.host}:${target.port}`,
        relay: target.relay,
      };
      const { dial } = await start(record, target);
      try {
        const snapshot = await dial;
        return { ...record, name: snapshot.host.name || record.name };
      } catch (e) {
        forget(target.hostKey);
        throw e;
      }
    },

    forget,

    get: (hostKey) => clients.get(hostKey),
  };
}
