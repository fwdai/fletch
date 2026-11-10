// Small key/value store for the paired hosts — each one's address, name, public
// key and relay URL — which of them is active, and the theme. App data dir via
// the fs plugin inside Tauri, localStorage in a browser. There is no credential
// here: the device's identity is the Noise static key, which lives in the Rust
// layer's app data dir and is the same for every host.

import { DEFAULT_PORT } from "@desktop/remote/types";
import { inTauri } from "@desktop/remote/ws";
import { BaseDirectory, exists, mkdir, readTextFile, writeTextFile } from "@tauri-apps/plugin-fs";

const FILE = "settings.json";
const DIR = "fletch";
const LS_KEY = "fletch-mobile-settings";

/** One paired host. The same record the desktop keeps for the hosts it pairs
 *  with (src/storage/remoteHosts.ts), except that the address stays split as
 *  `host` and `port` — the shape every connect target already has. */
export interface SavedHost {
  /** The host's public key, base64url — its identity and its route on the
   *  relay. Being on this list is what "paired" means. */
  hostKey: string;
  /** What to call it: the host's own name, from its last handshake. */
  name: string;
  host: string;
  port: number;
  /** Relay base URL, as the host last answered it. Absent means the LAN
   *  address is the only way in. */
  relay?: string;
  /** RFC3339, stamped when the host was first added. Nothing branches on it. */
  pairedAt: string;
}

export interface Persisted {
  /** Every paired host, in the order they were added. */
  hosts?: SavedHost[];
  /** The host the app connects to on launch. A key no longer on `hosts` (or
   *  none) falls back to the first one. */
  activeHostKey?: string;
  /** Where the last clone landed, keyed by host public key — the desktop
   *  remembers the same thing, and the folder only means something on the Mac
   *  that owns it. */
  destParents?: Record<string, string>;
  /** Host public key → what iOS answered when we asked for notification
   *  permission for it. Keyed by host because the prompt belongs to a pairing,
   *  and remembered so no host is ever asked twice. */
  pushPermission?: Record<string, "granted" | "denied" | "prompt">;
  theme?: "system" | "light" | "dark";
  /** Float projects with recent agent activity to the top of Home. Absent
   *  means on — the desktop sidebar's default. */
  activeFirst?: boolean;
}

/** The single host an older build kept at the top level of the file. */
interface LegacyHost {
  host?: string;
  port?: number;
  hostName?: string;
  hostKey?: string;
  relay?: string;
}

/** Read the file as this build understands it. One that still has the old
 *  single-host fields becomes a one-entry `hosts` list with that host active,
 *  so an upgrade keeps working with no re-pairing. The legacy fields are
 *  dropped either way; the next write leaves them out of the file. */
export function migrate(raw: Persisted & LegacyHost): Persisted {
  const { host, port, hostName, hostKey, relay, ...rest } = raw;
  // Without its key the old record was never a pairing (`init` ignored it).
  if (rest.hosts || !host || !hostKey) return rest;
  return {
    ...rest,
    hosts: [
      {
        hostKey,
        name: hostName || host,
        host,
        port: port ?? DEFAULT_PORT,
        ...(relay ? { relay } : {}),
        pairedAt: new Date().toISOString(),
      },
    ],
    activeHostKey: hostKey,
  };
}

let cache: Persisted | null = null;
/** The first read, shared: two callers racing it must not each parse the file
 *  and have the later one overwrite what the earlier one already changed. */
let loading: Promise<void> | null = null;

async function load(): Promise<void> {
  try {
    if (inTauri()) {
      const path = `${DIR}/${FILE}`;
      cache = (await exists(path, { baseDir: BaseDirectory.AppData }))
        ? migrate(JSON.parse(await readTextFile(path, { baseDir: BaseDirectory.AppData })))
        : {};
    } else {
      cache = migrate(JSON.parse(localStorage.getItem(LS_KEY) ?? "{}"));
    }
  } catch {
    cache = {};
  }
}

async function readAll(): Promise<Persisted> {
  if (cache) return cache;
  loading ??= load();
  await loading;
  return cache ?? {};
}

async function writeAll(next: Persisted): Promise<void> {
  cache = next;
  const text = JSON.stringify(next);
  try {
    if (inTauri()) {
      if (!(await exists(DIR, { baseDir: BaseDirectory.AppData }))) {
        await mkdir(DIR, { baseDir: BaseDirectory.AppData, recursive: true });
      }
      await writeTextFile(`${DIR}/${FILE}`, text, { baseDir: BaseDirectory.AppData });
    } else {
      localStorage.setItem(LS_KEY, text);
    }
  } catch {
    // Persistence is a convenience; a failed write must not break the session.
  }
}

/** Read-modify-write against the latest copy. `fn` sees the cache as it is
 *  after the read has settled, not as it was when this was called, and nothing
 *  awaits between reading it and replacing it — so two writes fired together
 *  (a relay update and a host save) both land. */
async function mutate(fn: (current: Persisted) => Persisted): Promise<Persisted> {
  await readAll();
  const next = fn(cache ?? {});
  await writeAll(next);
  return next;
}

export const loadSettings = readAll;

export function saveSettings(patch: Persisted): Promise<Persisted> {
  return mutate((current) => ({ ...current, ...patch }));
}

/** `host` added to `hosts`, or folded into the entry with its key — pairing a
 *  host again is an update in place, keeping its position and `pairedAt`. */
export function upsertHost(hosts: SavedHost[], host: Omit<SavedHost, "pairedAt">): SavedHost[] {
  const existing = hosts.find((h) => h.hostKey === host.hostKey);
  if (!existing) return [...hosts, { ...host, pairedAt: new Date().toISOString() }];
  return hosts.map((h) => (h === existing ? { ...host, pairedAt: existing.pairedAt } : h));
}

/** Save `host` (added, or updated by key) and make it the active one. Returns
 *  the list as saved. */
export async function saveHost(host: Omit<SavedHost, "pairedAt">): Promise<SavedHost[]> {
  const next = await mutate((current) => ({
    ...current,
    hosts: upsertHost(current.hosts ?? [], host),
    activeHostKey: host.hostKey,
  }));
  return next.hosts ?? [];
}

/** Change fields on the entry for `hostKey`. A host with no entry (forgotten,
 *  or not saved yet) is left alone rather than brought back. */
export async function updateHost(
  hostKey: string,
  patch: Partial<Omit<SavedHost, "hostKey" | "pairedAt">>,
): Promise<void> {
  await mutate((current) => ({
    ...current,
    hosts: current.hosts?.map((h) => (h.hostKey === hostKey ? { ...h, ...patch } : h)),
  }));
}

/** Which host to connect to on the next launch. */
export async function setActiveHost(hostKey: string): Promise<void> {
  await saveSettings({ activeHostKey: hostKey });
}

/** Forget a paired host — address, name, pinned key and relay — and keep the
 *  rest. Both per-host-key maps survive: the destination folders are
 *  meaningless to anyone else and useful again if this Mac is re-paired, and
 *  the notification answers record what iOS has already been asked, which
 *  forgetting does not undo. Returns the hosts that are left. */
export async function forgetHost(hostKey: string): Promise<SavedHost[]> {
  const next = await mutate(({ activeHostKey, ...current }) => ({
    ...current,
    hosts: (current.hosts ?? []).filter((h) => h.hostKey !== hostKey),
    ...(activeHostKey && activeHostKey !== hostKey ? { activeHostKey } : {}),
  }));
  return next.hosts ?? [];
}

/** The clone destination this host was last given, or null when it has none.
 *  Keyed by host key so re-pairing the same Mac gets its folder back. */
export async function loadDestParent(hostKey: string | null): Promise<string | null> {
  if (!hostKey) return null;
  return (await readAll()).destParents?.[hostKey] ?? null;
}

export async function saveDestParent(hostKey: string | null, parent: string): Promise<void> {
  if (!hostKey) return;
  await mutate((current) => ({
    ...current,
    destParents: { ...current.destParents, [hostKey]: parent },
  }));
}
