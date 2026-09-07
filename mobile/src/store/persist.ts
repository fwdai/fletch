// Small key/value store for the last host — its address, name and public key —
// and the theme. App data dir via the fs plugin inside Tauri, localStorage in a
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

/** Forget the paired host — address, name and pinned key — and keep the rest. */
export async function clearHost(): Promise<void> {
  const { theme } = await readAll();
  await writeAll(theme ? { theme } : {});
}
