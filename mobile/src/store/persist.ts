// Small key/value store for the last host — its address, name, public key and
// relay URL — and the theme. App data dir via the fs plugin inside Tauri, localStorage in a
// browser. There is no credential here: the device's identity is the Noise
// static key, which lives in the Rust layer's app data dir.

import { BaseDirectory, exists, mkdir, readTextFile, writeTextFile } from "@tauri-apps/plugin-fs";
import { inTauri } from "../remote/ws";

const FILE = "settings.json";
const DIR = "fletch";
const LS_KEY = "fletch-mobile-settings";

export interface Persisted {
  host?: string;
  port?: number;
  hostName?: string;
  /** The host's public key. Its presence is what "paired" means. */
  hostKey?: string;
  /** Relay base URL for this host, from the pairing link or the Host sheet. */
  relay?: string;
  /** Where the last clone landed, keyed by host public key — the desktop
   *  remembers the same thing, and the folder only means something on the Mac
   *  that owns it. */
  destParents?: Record<string, string>;
  theme?: "system" | "light" | "dark";
}

let cache: Persisted | null = null;

async function readAll(): Promise<Persisted> {
  if (cache) return cache;
  try {
    if (inTauri()) {
      const path = `${DIR}/${FILE}`;
      if (await exists(path, { baseDir: BaseDirectory.AppData })) {
        cache = JSON.parse(await readTextFile(path, { baseDir: BaseDirectory.AppData }));
      } else {
        cache = {};
      }
    } else {
      cache = JSON.parse(localStorage.getItem(LS_KEY) ?? "{}");
    }
  } catch {
    cache = {};
  }
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

export const loadSettings = readAll;

export async function saveSettings(patch: Persisted): Promise<Persisted> {
  const next = { ...(await readAll()), ...patch };
  await writeAll(next);
  return next;
}

/** The clone destination this host was last given, or null when it has none.
 *  Keyed by host key so re-pairing the same Mac gets its folder back. */
export async function loadDestParent(hostKey: string | null): Promise<string | null> {
  if (!hostKey) return null;
  return (await readAll()).destParents?.[hostKey] ?? null;
}

export async function saveDestParent(hostKey: string | null, parent: string): Promise<void> {
  if (!hostKey) return;
  const { destParents } = await readAll();
  await saveSettings({ destParents: { ...destParents, [hostKey]: parent } });
}

/** Forget the paired host — address, name, pinned key and relay — and keep the
 *  rest. The destination folders stay: they are keyed by host key, so they are
 *  meaningless to anyone else and useful again if this Mac is re-paired. */
export async function clearHost(): Promise<void> {
  const { theme, destParents } = await readAll();
  await writeAll({ ...(theme ? { theme } : {}), ...(destParents ? { destParents } : {}) });
}
