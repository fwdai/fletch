// What this desktop tells a host about itself when it pairs. The host records
// the name and the platform in its device list and ignores the rest (`pair`'s
// `device` object, src-tauri/src/remote/server.rs) — this is a label in the
// host's Settings pane, never a credential. The credential is the Noise static
// key the Rust layer holds.

import { getVersion } from "@tauri-apps/api/app";
import { IS_MAC, IS_WINDOWS } from "@/util/platform";
import type { DeviceInfo } from "./types";

/** No command exposes this Mac's Sharing name to the frontend — `machine_name`
 *  in src-tauri/src/remote/mod.rs is the host's own, private to Rust — so every
 *  desktop introduces itself the same way for now. */
const DEFAULT_NAME = "Fletch desktop";

const platform = () => (IS_MAC ? "macos" : IS_WINDOWS ? "windows" : "linux");

let cached: Promise<DeviceInfo> | null = null;

/** This device, resolved once. The version is a Tauri call, so outside the app
 *  (a browser dev loop) there is nothing to ask and the field goes empty —
 *  which the host tolerates, and nothing may branch on anyway
 *  (docs/remote-protocol.md, "Compatibility"). */
export function thisDevice(): Promise<DeviceInfo> {
  cached ??= getVersion()
    .catch(() => "")
    .then((appVersion) => ({ name: DEFAULT_NAME, platform: platform(), appVersion }));
  return cached;
}
