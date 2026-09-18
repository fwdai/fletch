// The app's one host registry, and the three things anything outside it wants:
// dial the saved hosts at startup, add one from a pairing link, forget one.
//
// This is the only module that puts the registry, the app store and the
// settings row together; `./registry` is the lifecycle on its own and knows
// about none of them.

import { forgetHost as forgetRecord, loadHosts, saveHost } from "@/storage/remoteHosts";
import { useAppStore } from "@/store";
import { LOCAL_ENVIRONMENT_ID } from "@/store/environments";
import { ProtocolClient } from "./client";
import { thisDevice } from "./device";
import { createHostRegistry, type PairTarget } from "./registry";
import { openWebSocket } from "./ws";

export const hosts = createHostRegistry({
  // Read on every write rather than captured: zustand's writers are stable, but
  // reaching for them through `getState` keeps this module from holding a
  // reference to the store's first state object.
  writers: {
    upsertEnvironment: (entry) => useAppStore.getState().upsertEnvironment(entry),
    setEnvironmentConnection: (id, connection, error, retrying) =>
      useAppStore.getState().setEnvironmentConnection(id, connection, error, retrying),
    removeEnvironment: (id) => useAppStore.getState().removeEnvironment(id),
    environmentReconnected: (id) => useAppStore.getState().environmentReconnected(id),
  },
  device: thisDevice,
  newClient: (device) => new ProtocolClient({ openSocket: openWebSocket, device }),
});

/** Dial every saved host, once, after the app has rendered.
 *
 *  Deferred by its caller (src/main.tsx) and cheap when there is nothing to do:
 *  the settings read is one `db_select` on a keyed row, and with no saved hosts
 *  it returns an empty array and this ends. Nothing here is awaited by anything
 *  the user is waiting for, and a host that is offline only ever occupies its
 *  own client's backoff. */
export async function startSavedHosts(): Promise<void> {
  const saved = await loadHosts().catch(() => []);
  // In parallel, and each swallowing its own failure: one unreachable host must
  // not hold up the next, and `adopt` has already put the reason on its entry.
  await Promise.all(saved.map((record) => hosts.adopt(record).catch(() => {})));
}

/** Pair with the host a link points at, then remember it and keep it
 *  connected. Rejects with the host's own words when the handshake fails. */
export async function addHost(target: PairTarget): Promise<void> {
  const record = await hosts.pair(target);
  await saveHost({ ...record, pairedAt: new Date().toISOString() });
}

/** Close the connection and drop the record. Idempotent.
 *
 *  Driving the UI back to This Mac FIRST when the host being forgotten is the
 *  one on screen: `removeEnvironment` only refuses to strand `activeEnvironmentId`
 *  on a missing id, it cannot put the local view back — that is
 *  `switchEnvironment`'s job, and it needs the entry to still be there to park
 *  the forgotten host's state on. */
export async function removeHost(hostKey: string): Promise<void> {
  const store = useAppStore.getState();
  if (store.activeEnvironmentId === hostKey) {
    await store.switchEnvironment(LOCAL_ENVIRONMENT_ID);
  }
  hosts.forget(hostKey);
  await forgetRecord(hostKey);
}
