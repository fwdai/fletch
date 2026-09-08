// Push registration — the phone's side of docs/remote-protocol.md, "Push
// notifications". Two jobs: ask iOS for permission once per paired host, and
// make sure the host always holds this phone's current APNs token. A tap on an
// alert is routed by the store's own `openFromPush`, because navigation is the
// router's business, not the plugin's.

import type { Api } from "../api";
import { ignore } from "../lib/ignore";
import type { RemoteClient } from "../remote";
import {
  onPushOpened,
  onPushToken,
  type PushPermission,
  type PushToken,
  registerPush,
  requestPushPermission,
} from "../remote/push";
import { inTauri } from "../remote/ws";
import type { MobileState } from "./index";
import { loadSettings, saveSettings } from "./persist";

export interface PushDeps {
  client: RemoteClient;
  api: Api;
  get: () => MobileState;
}

/** Set by `startPush`, and left null when there is no plugin to talk to — which
 *  is what makes every entry point below inert in a browser. */
let deps: PushDeps | null = null;

/** The last token iOS handed us, for the life of the process: the host has to
 *  be told again after every handshake, not only when the token changes. */
let token: PushToken | null = null;

/** Called at the top of `init`, before anything can trigger a registration: the
 *  plugin holds the token — and a tap that launched the app — until the first
 *  `register`, and releases both through these two events. */
export async function startPush(next: PushDeps): Promise<void> {
  if (!inTauri()) return;
  deps = next;
  await onPushToken((fresh) => {
    token = fresh;
    void report();
  });
  await onPushOpened((opened) => next.get().openFromPush(opened.fletch));

  // Apple wants a registration on every launch, and it is also what releases a
  // tap that arrived while the webview was still loading. Only where the user
  // has already agreed, though: asking is the pairing's job, below.
  const saved = await loadSettings();
  if (saved.hostKey && saved.pushPermission?.[saved.hostKey] === "granted") {
    await registerPush().catch(ignore);
  }
}

/** Called after every successful `pair` and `hello`. */
export async function syncPush(): Promise<void> {
  if (!deps) return;
  const hostKey = deps.client.hostKey ?? deps.get().hostKey;
  if (!hostKey) return;
  // A refusal is final: nothing is sent, so the host keeps no token for this
  // device and never asks the relay to reach it.
  if ((await permissionFor(hostKey)) !== "granted") return;
  await registerPush().catch(ignore);
  await report();
}

/** Ask iOS at most once per host. The answer is persisted, so neither a
 *  reconnect nor a relaunch re-prompts, and a denial is never revisited. */
async function permissionFor(hostKey: string): Promise<PushPermission> {
  const saved = (await loadSettings()).pushPermission?.[hostKey];
  if (saved) return saved;
  const state = await requestPushPermission();
  const settings = await loadSettings();
  await saveSettings({ pushPermission: { ...settings.pushPermission, [hostKey]: state } });
  return state;
}

/** The host takes lowercase hex and rejects anything else as an op error, so a
 *  token that does not look like one is dropped here rather than sent. */
const isDeviceToken = (value: string) => /^[0-9a-f]+$/.test(value);

/** Hand the host the token it should have the relay send to. There is nothing
 *  to say until iOS has given us one, and no way to say it until the link is
 *  up — the next handshake calls `syncPush` again. */
async function report(): Promise<void> {
  if (!deps || !token || deps.client.state !== "connected") return;
  if (!isDeviceToken(token.token)) return;
  await deps.api.registerPush(token.token, token.environment).catch(ignore);
}
