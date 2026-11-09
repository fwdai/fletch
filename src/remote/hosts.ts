// The app's one host registry, and the things anything outside it wants: dial
// the saved hosts at startup, add one from a pairing link, forget one, and pair
// back with a desktop that is a host too ("Pairing back").
//
// This is the only module that puts the registry, the app store and the
// settings row together; `./registry` is the lifecycle on its own and knows
// about none of them.

import { remoteApi } from "@/api/domains/remote";
import { forgetHost as forgetRecord, loadHosts, saveHost, updateHost } from "@/storage/remoteHosts";
import { useAppStore } from "@/store";
import { LOCAL_ENVIRONMENT_ID } from "@/store/environments";
import { ProtocolClient } from "./client";
import { thisDevice } from "./device";
import { pairTargetFrom } from "./pairing";
import { createHostRegistry, type HostRecord, type PairTarget } from "./registry";
import type { CodeHandler } from "./types";
import { openWebSocket } from "./ws";

export const hosts = createHostRegistry({
  // Read on every write rather than captured: zustand's writers are stable, but
  // reaching for them through `getState` keeps this module from holding a
  // reference to the store's first state object.
  writers: {
    upsertEnvironment: (entry) => useAppStore.getState().upsertEnvironment(entry),
    setEnvironmentConnection: (id, connection, error, retrying) =>
      useAppStore.getState().setEnvironmentConnection(id, connection, error, retrying),
    setEnvironmentProviders: (id, providers) =>
      useAppStore.getState().setEnvironmentProviders(id, providers),
    removeEnvironment: (id) => useAppStore.getState().removeEnvironment(id),
    environmentReconnected: (id) => useAppStore.getState().environmentReconnected(id),
  },
  device: thisDevice,
  newClient: (device) => new ProtocolClient({ openSocket: openWebSocket, device }),
  onRelayChange: (hostKey, relay) => void updateHost(hostKey, { relay }).catch(() => {}),
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
 *  connected. Rejects with the host's own words when the handshake fails.
 *  `onCode` hears the six digits of a confirmed pairing, after which the
 *  person here may be asked whether that host can control this Mac too. */
export async function addHost(target: PairTarget, onCode?: CodeHandler): Promise<void> {
  const record = await hosts.pair(target, onCode);
  await saveHost({ ...record, pairedAt: new Date().toISOString() });
  if (target.confirm && !target.pairingToken) await askToPairBack(record);
}

/** The op that hands a host this Mac's own pairing link. */
const OFFER_INVITE = "offer_invite";

/** After a confirmed pairing, put the pair-back question to the person here
 *  (docs/remote-protocol.md, "Pairing back") — only when there is something to
 *  offer: this Mac's own remote access is listening, and the host takes the
 *  offer from this device (`offer_invite` is in what it answered, which means
 *  a host that knows the op and a Full pairing). Nothing is minted here; that
 *  waits for Allow (`offerPairBack`). Never rejects: the pairing stands. */
async function askToPairBack(record: HostRecord): Promise<void> {
  if (!hosts.supports(record.hostKey, OFFER_INVITE)) return;
  const listening = await remoteApi.remoteStatus().then(
    (status) => status.listening,
    () => false,
  );
  if (listening) {
    useAppStore.getState().setPairBackOffer({ hostKey: record.hostKey, name: record.name });
  }
}

/** Let the host at `hostKey` control this Mac too, on the person's Allow: mint
 *  a Full pairing link here and hand it over with `offer_invite`, which the
 *  host pairs with by itself. Rejects with the reason. The link is single use,
 *  and lapses after five minutes if the host never redeems it. */
export async function offerPairBack(hostKey: string): Promise<void> {
  const { url } = await remoteApi.remoteBeginPairing("full");
  await hosts.call(hostKey, OFFER_INVITE, { url });
}

/** Pair with a link a paired desktop offered (`remote:invite-offered`). Its
 *  sender is a Full device of this Mac, which the person here accepted, handing
 *  over its own machine; so it is paired with like a pasted link. */
export async function pairWithOfferedLink(url: string): Promise<void> {
  const parsed = pairTargetFrom(url);
  if ("error" in parsed) throw new Error(parsed.error);
  await addHost(parsed.target);
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
