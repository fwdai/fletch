// What this desktop tells a host about itself when it pairs. The host records
// the name and the platform in its device list and ignores the rest (`pair`'s
// `device` object, crates/fletch-core/src/remote/server.rs) — this is a label in
// the host's Settings pane, never a credential. The credential is the Noise
// static key the Rust layer holds.

import { getVersion } from "@tauri-apps/api/app";
import { remoteApi } from "@/api/domains/remote";
import { IS_MAC, IS_WINDOWS } from "@/util/platform";
import type { DeviceInfo } from "./types";

/** What a device is called when this Mac cannot say — a browser dev loop, or a
 *  `remote_status` that failed (no host key yet, an unreadable remote dir). */
const DEFAULT_NAME = "Fletch desktop";

const platform = () => (IS_MAC ? "macos" : IS_WINDOWS ? "windows" : "linux");

let cached: Promise<DeviceInfo> | null = null;

/** This device, resolved once. Both facts are Tauri calls, so outside the app
 *  (a browser dev loop) there is nothing to ask: the version goes empty — which
 *  the host tolerates, and nothing may branch on anyway
 *  (docs/remote-protocol.md, "Compatibility") — and the name falls back to
 *  `DEFAULT_NAME`.
 *
 *  `remote_status` is this Mac's own remote surface, so the name it reports is
 *  this machine's (`remote::machine_name`), not a paired host's. Reading it
 *  does not require remote access to be switched on. */
export function thisDevice(): Promise<DeviceInfo> {
  cached ??= Promise.all([
    getVersion().catch(() => ""),
    remoteApi
      .remoteStatus()
      .then((s) => s.name.trim() || DEFAULT_NAME)
      .catch(() => DEFAULT_NAME),
  ]).then(([appVersion, name]) => ({ name, platform: platform(), appVersion }));
  return cached;
}
