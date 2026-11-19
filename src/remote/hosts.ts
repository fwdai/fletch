// The app's one host registry, and the things anything outside it wants: dial
// the saved hosts at startup, add one from a pairing link, forget one, and pair
// back with a desktop that is a host too ("Pairing back").
//
// This is the only module that puts the registry, the app store and the
// settings row together; `./registry` is the lifecycle on its own and knows
// about none of them.

import { remoteApi } from "@/api/domains/remote";
import { forgetHost as forgetRecord, loadHosts, saveHost, updateHost } from "@/storage/remoteHosts";
import { hostStore, startHost, stopHost, useAppStore } from "@/store";
import { ProtocolClient } from "./client";
import { thisDevice } from "./device";
import { localHostname, pairTargetFrom } from "./pairing";
import { createHostRegistry, type HostRecord, type PairTarget } from "./registry";
import type { CodeHandler } from "./types";
import { openWebSocket } from "./ws";

export const hosts = createHostRegistry({
  // Read on every write rather than captured: zustand's writers are stable, but
  // reaching for them through `getState` keeps this module from holding a
  // reference to the store's first state object.
  writers: {
    // The entry that carries a transport is the one published as the host's
    // client is created: from then on the host is live — its listeners fold
    // into its partition whether or not it is on screen.
    upsertEnvironment: (entry) => {
      useAppStore.getState().upsertEnvironment(entry);
      if (entry.transport) void startHost(entry.id);
    },
    setEnvironmentConnection: (id, connection, error, retrying) =>
      useAppStore.getState().setEnvironmentConnection(id, connection, error, retrying),
    setEnvironmentProviders: (id, providers) =>
      useAppStore.getState().setEnvironmentProviders(id, providers),
    removeEnvironment: (id) => {
      void stopHost(id);
      useAppStore.getState().removeEnvironment(id);
    },
    // The host's own actions re-read its own partition.
    environmentReconnected: (id) => hostStore(id).getState().environmentReconnected(id),
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
  let record: HostRecord;
  try {
    record = await hosts.pair(target, onCode);
  } catch (e) {
    // Pairing again with a host already saved (an expired link, or one that
    // changes its access) took its working connection down first. Its record
    // is untouched, so it is dialled again from that, as at launch — unless a
    // newer pairing with it is running, which `adopt` leaves alone.
    const saved = (await loadHosts().catch(() => [])).find((h) => h.hostKey === target.hostKey);
    if (saved) await hosts.adopt(saved).catch(() => {});
    throw e;
  }
  await saveHost({ ...record, pairedAt: new Date().toISOString() });
  if (target.confirm && !target.pairingToken) await askToPairBack(record);
}

/** Remember a host this device is already authorised on and keep it
 *  connected, as a pairing would but with no code: the cloud runner this Mac
 *  created, whose engine added this device's key while setting it up.
 *
 *  It is reached through its relay alone; no network this Mac is on has an
 *  address for it. Its saved address is the name it would announce on one
 *  (`localHostname`), which parses like any other, is dialled first like any
 *  host's LAN address, and is answered by nothing, so each dial moves on to
 *  the relay after the LAN's open timeout, as a phone off a Mac's network
 *  does. */
export async function addRelayHost(host: {
  hostKey: string;
  name: string;
  relay: string;
}): Promise<void> {
  const record: HostRecord = { ...host, addr: localHostname(host.hostKey) ?? "" };
  await saveHost({ ...record, pairedAt: new Date().toISOString() });
  await hosts.adopt(record);
}

/** The op that hands a host this Mac's own pairing link. */
const OFFER_INVITE = "offer_invite";

/** After a confirmed pairing, put the pair-back question to the person here
 *  (docs/remote-protocol.md, "Pairing back") — only when the host takes the
 *  offer from this device (`offer_invite` is in what it answered, which means
 *  a host that knows the op and a Full pairing) and this Mac can be a host at
 *  all (its device store works). A Mac whose remote control is off is asked
 *  too, and told that Allow turns it on. Nothing is minted or switched on
 *  here; that waits for Allow (`offerPairBack`). Never rejects: the pairing
 *  stands. */
async function askToPairBack(record: HostRecord): Promise<void> {
  if (!hosts.supports(record.hostKey, OFFER_INVITE)) return;
  const status = await remoteApi.remoteStatus().catch(() => null);
  if (!status || status.error) return;
  useAppStore.getState().setPairBackOffer({
    hostKey: record.hostKey,
    name: record.name,
    enables: !status.listening,
  });
}

/** Let the host at `hostKey` control this Mac too, on the person's Allow: turn
 *  this Mac's remote control on if it is off, mint a Full pairing link here
 *  and hand it over with `offer_invite`, which the host pairs with by itself.
 *  Rejects with the reason. The link is single use, and lapses after five
 *  minutes if the host never redeems it. */
export async function offerPairBack(hostKey: string): Promise<void> {
  const status = await remoteApi.remoteStatus();
  if (!status.listening) {
    const now = await remoteApi.remoteSetEnabled(true);
    if (!now.listening) throw new Error(now.error ?? "Couldn't turn on remote control here.");
  }
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

/** Close the connection and drop the record. Idempotent. Forgetting the host
 *  on screen puts This Mac on screen (`removeEnvironment`). */
export async function removeHost(hostKey: string): Promise<void> {
  hosts.forget(hostKey);
  await forgetRecord(hostKey);
}
