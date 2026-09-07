// Small key/value store for the device token, the last host and the theme.
// App data dir via the fs plugin inside Tauri, localStorage in a browser.
// Keychain is a follow-up (docs/remote-protocol.md, out of scope for v1).

import { BaseDirectory, exists, mkdir, readTextFile, writeTextFile } from "@tauri-apps/plugin-fs";
import { inTauri } from "../remote/ws";

const FILE = "settings.json";
const DIR = "fletch";
const LS_KEY = "fletch-mobile-settings";

export interface Persisted {
  host?: string;
  port?: number;
  hostName?: string;
  deviceToken?: string;
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

export async function clearCredentials(): Promise<void> {
  const { theme } = await readAll();
  await writeAll(theme ? { theme } : {});
}
