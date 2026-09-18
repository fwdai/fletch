// The hosts this desktop has paired with, in the settings table the rest of the
// app's preferences already live in (`remote.hosts`, one JSON array).
//
// There is no credential here. This desktop's identity as a *client* is the
// Noise static key the Rust layer holds, so a record is only where to dial and
// what to call it — the same fields the phone persists
// (mobile/src/store/persist.ts), keyed by host public key because that key is
// the host's identity and therefore the environment id.

import { getSetting, setSetting } from "./settings";

export const REMOTE_HOSTS_KEY = "remote.hosts";

export interface SavedHost {
  /** The host's Noise static public key, base64url — its identity, its route
   *  on the relay, and the environment id in the store. */
  hostKey: string;
  /** What to call it: the host's own name, from the handshake that paired it. */
  name: string;
  /** `<ip-or-name>:<port>`, exactly as the pairing link carried it. Parsed back
   *  with `parseAddress`, so a bare address (no port) is accepted too. */
  addr: string;
  /** Relay base URL, when the host had one configured at pairing. Absent means
   *  the LAN address is the only way in. */
  relay?: string;
  /** RFC3339. Shown in the pane; nothing branches on it. */
  pairedAt: string;
}

const isHost = (v: unknown): v is SavedHost => {
  const h = v as SavedHost | null;
  return (
    !!h &&
    typeof h === "object" &&
    typeof h.hostKey === "string" &&
    h.hostKey.length > 0 &&
    typeof h.addr === "string"
  );
};

/** Every saved host. A missing, unparseable or non-array value reads as none,
 *  and a corrupt entry is skipped rather than losing its neighbours — the same
 *  rule the host's own device store follows. */
export async function loadHosts(): Promise<SavedHost[]> {
  try {
    const raw = await getSetting(REMOTE_HOSTS_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter(isHost) : [];
  } catch {
    return [];
  }
}

/** Add `host`, or replace the record with the same key — re-pairing a host is
 *  an update, not a second entry. Returns the list as saved. */
export async function saveHost(host: SavedHost): Promise<SavedHost[]> {
  const next = [...(await loadHosts()).filter((h) => h.hostKey !== host.hostKey), host];
  await setSetting(REMOTE_HOSTS_KEY, next);
  return next;
}

/** Drop the record for `hostKey`. Returns the list as saved. */
export async function forgetHost(hostKey: string): Promise<SavedHost[]> {
  const next = (await loadHosts()).filter((h) => h.hostKey !== hostKey);
  await setSetting(REMOTE_HOSTS_KEY, next);
  return next;
}
