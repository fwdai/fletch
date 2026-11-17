// Push registration — the phone's side of docs/remote-protocol.md, "Push
// notifications". Two jobs: ask iOS for permission once per paired host, and
// make sure every paired host always holds this phone's current APNs token,
// each told over its own link — on its own handshakes, and when the token
// changes. A tap on an alert is routed by the store's own `openFromPush`,
// because navigation is the router's business, not the plugin's.

import { inTauri } from "@desktop/remote/ws";
import { type HostLink, linkOf } from "../host";
import { ignore } from "../lib/ignore";
import { withTimeout } from "../lib/timeout";
import {
  onPushOpened,
  onPushToken,
  type PushPermission,
  type PushToken,
  registerPush,
  requestPushPermission,
} from "../remote/push";
import type { AgentTab, AppState } from "./index";
import { loadSettings, saveSettings } from "./persist";

/** Push `kind`s about the agent's PR (docs/remote-protocol.md, "Push
 *  notifications"): the ship loop's, and autopilot giving up on one. */
const SHIP_PUSH_KINDS = new Set([
  "checks_settled",
  "review_comment",
  "pr_merged",
  "pr_closed",
  "autopilot_gave_up",
]);

/** The agent tab a tapped alert of `kind` opens on: Ship for an alert about the
 *  PR, the chat (undefined) for a turn ending or a held prompt. */
export const pushTab = (kind: string | undefined): AgentTab | undefined =>
  SHIP_PUSH_KINDS.has(kind ?? "") ? "ship" : undefined;

export interface PushDeps {
  get: () => AppState;
}

/** Set by `startPush`, and left null when there is no plugin to talk to — which
 *  is what makes every entry point below inert in a browser. */
let deps: PushDeps | null = null;

/** The last token iOS handed us, for the life of the process: a host has to be
 *  told again after every handshake of its own, not only when the token
 *  changes. */
let token: PushToken | null = null;

/** The plugin's registration this run, once asked for. Apple wants one per
 *  launch, and iOS hands the token back (`onPushToken`) on every one, so
 *  asking again would only repeat it. A failed one is asked for again. */
let registration: Promise<unknown> | null = null;

const register = () =>
  (registration ??= registerPush().catch(() => {
    registration = null;
  }));

/** Called at the top of `init`, before anything can trigger a registration: the
 *  plugin holds the token — and a tap that launched the app — until the first
 *  `register`, and releases both through these two events. */
export async function startPush(next: PushDeps): Promise<void> {
  if (!inTauri()) return;
  deps = next;
  await onPushToken((fresh) => {
    const same = fresh.token === token?.token && fresh.environment === token?.environment;
    token = fresh;
    // A new token is every paired host's business; the same one again is
    // nobody's — each host was told it on its own handshake (`syncPush`).
    if (same) return;
    for (const { hostKey } of next.get().hosts) void report(linkOf(hostKey));
  });
  await onPushOpened((opened) => next.get().openFromPush(opened.fletch));

  // Apple wants a registration on every launch, and it is also what releases a
  // tap that arrived while the webview was still loading. Only where the user
  // has already agreed — for any paired host, since every one of them holds
  // the token and may alert — though: asking is the pairing's job, below.
  const saved = await loadSettings();
  if (saved.hosts?.some((h) => saved.pushPermission?.[h.hostKey] === "granted")) {
    await register();
  }
}

/** Called after every successful `pair` and `hello` on `link`: that host, and
 *  no other, is told the token. */
export async function syncPush(link: HostLink): Promise<void> {
  if (!deps) return;
  const hostKey = link.client.hostKey ?? link.hostKey;
  if (!hostKey) return;
  // A refusal is final: nothing is sent, so the host keeps no token for this
  // device and never asks the relay to reach it.
  if ((await permissionFor(hostKey)) !== "granted") return;
  // The first host to be granted registers the app; the token it releases
  // reaches this host with every other (`onPushToken` above).
  await register();
  await report(link);
}

/** Called when a host is forgotten, over its link while that is still up: the
 *  host only drops this phone's token on an explicit `{ token: null }`, so
 *  leaving without one would let a Mac this phone has forgotten keep alerting
 *  it. Best effort — forgetting must not be blocked by a host that is already
 *  gone. */
export async function forgetPush(link: HostLink): Promise<void> {
  if (!deps || link.client.state !== "connected") return;
  // Bounded: remote calls have no timeout, and a host that has stopped
  // answering without closing the socket must not hold the unpair hostage.
  await withTimeout(link.api.registerPush(null).catch(ignore), FORGET_PUSH_TIMEOUT_MS);
}

/** How long an unpair waits for the host to acknowledge the token clear. */
export const FORGET_PUSH_TIMEOUT_MS = 2000;

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

/** Hand `link`'s host the token it should have the relay send to. There is
 *  nothing to say until iOS has given us one, no way to say it until the link
 *  is up — the next handshake calls `syncPush` again — and nothing to say to a
 *  host whose alerts the user refused. */
async function report(link: HostLink): Promise<void> {
  const fresh = token;
  const hostKey = link.client.hostKey ?? link.hostKey;
  if (!fresh || !hostKey || link.client.state !== "connected") return;
  if (!isDeviceToken(fresh.token)) return;
  if ((await loadSettings()).pushPermission?.[hostKey] !== "granted") return;
  await link.api.registerPush(fresh.token, fresh.environment).catch(ignore);
}
