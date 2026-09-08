// The webview's half of the push plugin (mobile/src-tauri/plugins/push): three
// commands and two events, described in docs/remote-protocol.md under "Push
// notifications". Outside Tauri there is no plugin — and no APNs — so every
// call here answers as if permission had been refused, which leaves the browser
// dev loop with nothing to register and nobody to prompt.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { inTauri } from "./ws";

export type PushPermission = "granted" | "denied" | "prompt";

export interface PushToken {
  /** The APNs device token, lowercase hex. */
  token: string;
  /** Which APNs host reaches this build — the relay needs to know. */
  environment: "sandbox" | "production";
}

/** The `fletch` object the relay puts next to `aps` in every alert. It comes
 *  off the wire, so nothing in it is guaranteed. */
export interface PushFletch {
  hostId?: string;
  agentId?: string;
  kind?: string;
}

export interface PushOpened {
  fletch: PushFletch;
}

const command = <T>(name: string, offline: T): Promise<T> =>
  inTauri() ? invoke<T>(`plugin:push|${name}`) : Promise.resolve(offline);

/** iOS prompts at most once per install; the app asks after a pairing, never
 *  on launch. */
export const requestPushPermission = () => command<PushPermission>("request_permission", "denied");

/** Register with APNs. The token does not come back from here — it arrives as
 *  `push://token`, along with any token or tap the plugin has been holding
 *  since launch, so attach the listeners below first. */
export const registerPush = () => command<null>("register", null);

export const unregisterPush = () => command<null>("unregister", null);

const on = <T>(event: string, handle: (payload: T) => void): Promise<UnlistenFn> =>
  inTauri() ? listen<T>(event, (e) => handle(e.payload)) : Promise.resolve(() => {});

export const onPushToken = (handle: (token: PushToken) => void) =>
  on<PushToken>("push://token", handle);

export const onPushOpened = (handle: (opened: PushOpened) => void) =>
  on<PushOpened>("push://opened", handle);
