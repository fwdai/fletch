// The app store: what belongs to the phone itself — the theme, navigation and
// sheets, the hosts it is paired with, which one is on screen, and a pairing in
// progress. Everything about a host lives in that host's store (store/host),
// one per link; this one decides which link is live and dials it.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import type { RawEvent } from "../adapters";
import { dropLink, type HostLink, linkOf, pairingLink } from "../host";
import { ignore } from "../lib/ignore";
import { withTimeout } from "../lib/timeout";
import {
  DEFAULT_PORT,
  type HostInfo,
  type HostTarget,
  mockEnabled,
  type PairStep,
} from "../remote";
import type { PushFletch } from "../remote/push";
import {
  type HostState,
  type HostStore,
  hostStoreOf,
  refreshChats,
  storeOfLink,
  useHostStore,
} from "./host";
import {
  forgetHost as forgetSavedHost,
  loadDestParent,
  loadSettings,
  type SavedHost,
  saveHost,
  saveSettings,
  setActiveHost,
  updateHost,
  upsertHost,
} from "./persist";
import { forgetPush, pushTab, startPush, syncPush } from "./push";

export {
  type AgentSource,
  agentOf,
  createHostStore,
  type HostState,
  type HostStore,
  hostStoreOf,
  projectOf,
  type SpawnInput,
  storeOfLink,
  useHostStore,
} from "./host";

export type ThemeMode = "system" | "light" | "dark";
export type ScreenName =
  | "home"
  | "project"
  | "agent"
  | "subagent"
  | "code"
  | "file"
  | "diff"
  | "providers";
export type SheetName =
  | "host"
  | "newAgent"
  | "newPlan"
  | "addProject"
  | "agentMore"
  | "shipMore"
  | "modelPicker"
  | "accountPicker"
  | "pr";

export interface NavItem {
  key: number;
  screen: ScreenName;
  props: Record<string, string>;
  phase: "enter" | "idle" | "leave";
}

/** The agent screen's tabs, as its `tab` nav prop names them. */
export type AgentTab = "chat" | "changes" | "ship";

export interface SheetState {
  name: SheetName;
  props: Record<string, string>;
  open: boolean;
}

export interface AppState {
  ready: boolean;
  /** How far the `connect` in flight has got, and null when none is.
   *
   *  Deliberately wider than the client's `connected`, which arrives as soon
   *  as `pair` is answered with the workspace still to fetch: a Pair screen
   *  that goes idle there has told the user it gave up while it is in fact
   *  most of the way through. It is also the whole progress display — the
   *  waits are long enough (a LAN dial timing out, then a relay) that a
   *  screen showing nothing reads as a hung app. */
  pairStep: PairStep | null;
  /** The six digits of a confirmed pairing in progress — what the Pair screen
   *  shows while the Mac decides. Null outside one. */
  pairCode: string | null;
  /** The `fletch://pair` link the app was opened with, so the Pair screen can
   *  show which Mac it is pairing with rather than an empty form — and keep
   *  the details for a one-tap retry if it fails. */
  pairTarget: HostTarget | null;
  /** Every host this phone is paired with (store/persist). One is live at a
   *  time — `hostKey` — and the Home header switches between them. */
  hosts: SavedHost[];
  /** The active host's public key — pinned on first contact, and what makes
   *  the app paired at all. */
  hostKey: string | null;
  /** The Pair screen is up to add another host over a paired one, rather than
   *  because there is none. Cleared by a pairing that lands, or by cancelling
   *  back to the active host. */
  addingHost: boolean;
  /** The link the app is live on: the one `connect` last dialled, and the only
   *  one with a connection — going live on another drops this one's first
   *  (`goLive`). Its store (`liveHost`) is the one on screen.
   *
   *  Not always the active host's link: between a pairing's handshake and
   *  `adopt` it is the link of the host that answered while `hostKey` still
   *  names the one before it, and while the Pair screen is up it is the
   *  pairing's. Before the first dial it is a link to no host, whose calls
   *  fail as a dropped link's do. */
  liveLink: HostLink;

  theme: ThemeMode;
  systemTheme: "light" | "dark";
  /** Home lists projects with recent agent activity first (see
   *  `lib/activeProjects`). Off keeps the host's order. */
  activeFirst: boolean;
  nav: NavItem[];
  sheet: SheetState | null;
  lastError: string | null;

  init(): Promise<void>;
  connect(target: HostTarget): Promise<void>;
  /** Pair from a `fletch://pair` deep link: show it on the Pair screen and
   *  connect. The host it names is added (or updated, if already paired) and
   *  becomes active. Ignores a repeat of the link already being paired. */
  pairFromLink(target: HostTarget): void;
  /** Make a paired host the live one: drop the current connection and greet
   *  `hostKey` with `hello` — this device is already registered there. */
  switchHost(hostKey: string): Promise<void>;
  /** Forget the active host (the Host sheet's Unpair): it is told to drop the
   *  push token first, and the app moves on to the next paired host, if any.
   *  Only the active host can be forgotten, because only it can be told. */
  unpair(): Promise<void>;
  /** Open the Pair screen to add another host. */
  addHost(): void;
  /** Leave the Pair screen without adding one, back on the active host. */
  cancelAddHost(): Promise<void>;
  setTheme(theme: ThemeMode): void;
  setSystemTheme(t: "light" | "dark"): void;
  setActiveFirst(on: boolean): void;
  clearError(): void;

  push(screen: ScreenName, props?: Record<string, string>): void;
  pop(): void;
  openFromPush(fletch: PushFletch): void;
  openSheet(name: SheetName, props?: Record<string, string>): void;
  /** Close whatever sheet is open. Safe to hand straight to an `onClose` or
   *  `onClick`: it ignores its arguments. */
  closeSheet(): void;
  /** Close the sheet only if `name` is the one showing: for an async action
   *  finishing late, which must not dismiss whatever the user opened since. */
  closeSheetIf(name: SheetName): void;
}

let initialized = false;

/** The links the app listens to. Each is listened to once, however often it
 *  goes live: a second registration would act on every handshake twice. */
const listened = new WeakSet<HostLink>();

/** How long a foregrounded app waits for the host to answer the workspace
 *  read before treating the socket as dead and reconnecting. `get_workspace`
 *  is one database read and answers in well under a second even over the
 *  relay, so a miss this long is the socket, not the host being busy. */
export const RESUME_PROBE_TIMEOUT_MS = 6_000;

/** A tapped alert that arrived before `init` finished. It cannot be acted on
 *  yet: which screen it opens depends on the host key, which is still being
 *  read off disk. */
let queuedPush: PushFletch | null = null;

/** A pairing link that arrived before `init` finished — the usual case, in
 *  fact: `registerDeepLinks` and `init` start together, and the link has only
 *  two plugin calls to wait on where `init` has the push registration and the
 *  settings read.
 *
 *  It cannot be acted on yet either, for two reasons. `init` finishes by
 *  reconnecting the saved host, which would tear this pairing down mid-flight
 *  and spend a code that is single use; and it publishes the settings it read
 *  before that, which on a fresh install means a null host key written over
 *  the one a pairing that got in first had just pinned. */
let queuedLink: HostTarget | null = null;

const homeItem = (): NavItem => ({ key: Date.now(), screen: "home", props: {}, phase: "idle" });

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** The active host's saved entry, or null when there is none (or it has just
 *  been forgotten). Returns the entry itself, so it is a stable selector. */
export const activeHostOf = (s: Pick<AppState, "hosts" | "hostKey">): SavedHost | null =>
  s.hosts.find((h) => h.hostKey === s.hostKey) ?? null;

/** What to call the active host. The saved name first: the client's idea of
 *  the host lags a switch until the new one has answered, and the saved one is
 *  kept current by every handshake. */
export function useActiveHostName(): string | null {
  const saved = useApp((s) => activeHostOf(s)?.name);
  const answered = useHostStore((s) => s.hostInfo?.name);
  return saved ?? answered ?? null;
}

/** Whether `agentId`'s screen is on the stack — the one case a transcript read
 *  for it has anyone to show it to. */
export const isAgentOpen = (s: Pick<AppState, "nav">, agentId: string): boolean =>
  s.nav.some((n) => n.props.agentId === agentId);

/** Where to dial a saved host. Its key is the whole credential: `hello`
 *  authenticates with the device key the Rust layer holds, and the relay is
 *  dialled only if the LAN address does not answer. */
const targetOf = (saved: SavedHost): HostTarget => ({
  host: saved.host,
  port: saved.port,
  name: saved.name,
  hostKey: saved.hostKey,
  relay: saved.relay,
});

/** The log of an open thread is kept current by live events, which a dropped
 *  socket or a backgrounded webview silently misses — so every reconnect and
 *  every return to the foreground re-reads it from the host. */
function refreshOpenAgent(host: HostState): void {
  const agentId = [...appStore.getState().nav].reverse().find((n) => n.props.agentId)?.props
    .agentId;
  if (agentId) void host.loadAgent(agentId).catch(ignore);
}

/** The Providers screen, when it is up: its selection and meters follow
 *  `settings:changed`, which a dropped socket or a suspended app misses like
 *  any other event, and the handshake carries none of it. */
function refreshProviders(host: HostState): void {
  if (appStore.getState().nav.some((n) => n.screen === "providers")) {
    void host.loadProviderAccounts().catch(ignore);
  }
}

type Set = (partial: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => void;
type Get = () => AppState;

/** Everything that makes the app paired with the host it has just greeted: the
 *  pinned key, the path the link came in on, and the durable record of both —
 *  added to the paired hosts, or updated there, and made the active one.
 *
 *  Shared by `connect` and the snapshot subscription because a first
 *  connection can complete through either. A pairing that is answered and then
 *  loses the socket before the workspace arrives has already spent its code,
 *  and the client retries it on its own with the key it pinned — that retry
 *  has no `connect` above it, and without this the app would sit on the Pair
 *  screen holding a working connection. */
async function adopt(set: Set, get: Get, link: HostLink, host: HostInfo): Promise<void> {
  const { client } = link;
  // The client owns the target: the spent pairing token is gone from it and
  // the host key the handshake authenticated is pinned in.
  const target = client.target;
  const hostKey = client.hostKey ?? get().hostKey;
  const relay = target?.relay ?? null;
  const via = client.via;
  const entry =
    target && hostKey
      ? {
          hostKey,
          name: host.name || target.name || target.host,
          host: target.host,
          port: target.port,
          ...(relay ? { relay } : {}),
        }
      : null;
  // The remembered clone destination is per host, so it can only be
  // resolved once the handshake says which host this is.
  const lastDestParent = await loadDestParent(hostKey);
  storeOfLink(link).setState({ hostInfo: host, relay, via, lastDestParent });
  set({
    hostKey,
    nav: [homeItem()],
    addingHost: false,
    ...(entry ? { hosts: upsertHost(get().hosts, entry) } : {}),
  });
  // Mock mode must not leave a "mock" host behind for the next real run.
  if (mockEnabled() || !entry) return;
  await saveHost(entry);
}

/** Make `link` the live one: listened to, and the only link with a
 *  connection. Listened to before it is live, so nothing its subscription
 *  hears in between is taken for news. */
function goLive(set: Set, get: Get, link: HostLink): void {
  if (!listened.has(link)) {
    listened.add(link);
    listen(set, get, link);
  }
  const left = get().liveLink;
  if (link === left) return;
  set({ liveLink: link });
  left.client.disconnect();
}

/** What `link`'s pairing steps and handshakes mean for the app. Only the live
 *  link speaks for the screen: a link stops being live by being disconnected,
 *  so all it can still say is that disconnect — which must not land over the
 *  state of the link taking its place. Its host events and connection state
 *  are its store's own (store/host). */
function listen(set: Set, get: Get, link: HostLink): void {
  const { client } = link;
  const isLive = () => link === get().liveLink;
  // Only while a `connect` owns the flow: a background reconnect is the
  // connection banner's business, and moving the Pair screen's progress on
  // behalf of one would be describing a connection the user did not ask for.
  client.onStep((step) => {
    if (isLive() && get().pairStep) set({ pairStep: step });
  });
  client.onConfirmCode((code) => {
    if (isLive() && get().pairStep) set({ pairCode: code });
  });
  client.onSnapshot((snapshot) => {
    if (!isLive()) return;
    const host = storeOfLink(link);
    // A host other than the one on screen answered — a pairing that adds
    // one, or the retry that completes it. Nothing its store holds is to be
    // shown, and `adopt` (in `connect`, or below) makes it the active host.
    const newHost = client.hostKey !== get().hostKey;
    if (newHost) {
      host.getState().clear();
      host.setState({ via: client.via });
    }
    host.getState().applyHandshake(snapshot);
    // The client has folded the host's relay into its target; a reconnect
    // that never passes through `adopt` still has to keep it — and the name
    // the host answers to now, for the header.
    const relay = client.target?.relay ?? null;
    const saved = newHost ? null : activeHostOf(get());
    if (saved) {
      if (relay !== host.getState().relay) host.setState({ relay });
      const patch = { relay: relay ?? undefined, name: snapshot.host.name || saved.name };
      if (patch.relay !== saved.relay || patch.name !== saved.name) {
        set((s) => ({ hosts: s.hosts.map((h) => (h === saved ? { ...h, ...patch } : h)) }));
        if (!mockEnabled()) void updateHost(saved.hostKey, patch);
      }
    }
    refreshOpenAgent(host.getState());
    refreshProviders(host.getState());
    // Every handshake — the first pairing and every reconnect — is when the
    // host is told the APNs token again.
    void syncPush().catch(ignore);
    // A handshake from a host that is not the active one (or with none
    // active yet) is the one that pairs it: `connect` does this itself, so
    // this is for the retry that completes a pairing whose first attempt died
    // after the code was spent. Guarded on `pairStep` so the two never race.
    if (newHost && !get().pairStep) void adopt(set, get, link, snapshot.host).catch(ignore);
  });
}

export const appStore = createStore<AppState>()((set, get) => {
  /** The link the app is live on (`liveLink`). */
  const hostLink = (): HostLink => get().liveLink;

  return {
    ready: false,
    pairStep: null,
    pairCode: null,
    pairTarget: null,
    hosts: [],
    hostKey: null,
    addingHost: false,
    liveLink: pairingLink(),

    theme: "dark",
    systemTheme: "dark",
    activeFirst: true,
    nav: [{ key: 1, screen: "home", props: {}, phase: "idle" }],
    sheet: null,
    lastError: null,

    async init() {
      // Idempotent: React StrictMode mounts effects twice in dev, and a second
      // run would attach the push and foreground listeners twice.
      if (initialized) return;
      initialized = true;
      // Early, and awaited: the plugin holds the APNs token — and a tap that
      // launched the app — until it is asked to register, and hands both over
      // through events these listeners have to be in place for.
      await startPush({ link: hostLink, get }).catch(ignore);
      // Back in the foreground: refresh what the background missed — once the
      // socket has shown it is still alive. iOS freezes a suspended app's
      // socket, and when the app returns it can read as connected for a while
      // longer with every request on it hanging. So the refresh doubles as the
      // probe: a workspace read the host does not answer in time, or that the
      // socket refuses outright, means the socket is dead, and the reconnect it
      // triggers does the refreshing instead, through the handshake's snapshot.
      // The transport's own pings catch the same death for an idle foreground
      // app, more slowly. Whatever follows the probe acts on the host live by
      // then.
      const resume = async () => {
        const alive = await withTimeout(
          liveHost().getState().refreshWorkspace(),
          RESUME_PROBE_TIMEOUT_MS,
          false,
        );
        const host = liveHost().getState();
        if (!alive) {
          void host.reconnect().catch(ignore);
          return;
        }
        void host.loadShortstats();
        // A `pr:*` event that reached a suspended app may never have been read.
        void host.loadPrStatus();
        refreshOpenAgent(host);
        refreshProviders(host);
        refreshChats(host);
      };
      if (typeof document !== "undefined") {
        document.addEventListener("visibilitychange", () => {
          if (document.hidden) return;
          const { client } = hostLink();
          // A retry waiting out its backoff was scheduled while the phone was
          // away; the user looking at the app is reason enough to dial now.
          if (client.retrying) {
            void liveHost().getState().reconnect().catch(ignore);
            return;
          }
          if (client.state !== "connected") return;
          void resume();
        });
      }
      const saved = await loadSettings();
      // Mock mode keeps to its one in-memory host: real hosts saved by an
      // earlier run would only dial the mock under their names.
      const hosts = mockEnabled() ? [] : (saved.hosts ?? []);
      const active = hosts.find((h) => h.hostKey === saved.activeHostKey) ?? hosts[0] ?? null;
      const hostKey = active?.hostKey ?? null;
      // The pinned key comes back before `ready`, because it is what a queued
      // notification tap has to be checked against — a tap must not wait out a
      // connection attempt to open its agent.
      if (active) hostStoreOf(active.hostKey).setState({ relay: active.relay ?? null });
      set({
        ready: true,
        theme: saved.theme ?? "dark",
        activeFirst: saved.activeFirst ?? true,
        hosts,
        hostKey,
      });
      if (queuedPush) {
        const fletch = queuedPush;
        queuedPush = null;
        get().openFromPush(fletch);
      }
      // Before the saved host, and instead of it: a scanned link says to pair
      // with *this* Mac, which is an instruction, where reconnecting the last
      // one is only a default. Racing them would supersede the pairing and burn
      // its code. Over a paired host the Pair screen shows it as an addition,
      // with a way back to that host if it fails.
      if (queuedLink) {
        const target = queuedLink;
        queuedLink = null;
        set({ addingHost: hostKey !== null });
        await get().connect(target).catch(ignore);
        return;
      }
      if (mockEnabled()) {
        // The mock host has no pairing step worth clicking through every reload;
        // its fixed key is pinned by `connect` like any other.
        await get().connect({ host: "mock", port: DEFAULT_PORT }).catch(ignore);
        return;
      }
      // Stood down while a pairing is running, for the same reason the queued
      // link goes first: a link that arrived in the moment between `ready` and
      // here is already connecting, and this would replace it.
      if (active && !get().pairStep) {
        hostStoreOf(active.hostKey).setState({
          lastDestParent: saved.destParents?.[active.hostKey] ?? null,
        });
        await get().connect(targetOf(active)).catch(ignore);
      }
    },

    async connect(target) {
      // A host the target names has its own link; an address typed by hand gets
      // a fresh one, which becomes that host's link once it answers — and its
      // store that host's store.
      const link = target.hostKey ? linkOf(target.hostKey) : pairingLink();
      const host = storeOfLink(link);
      // The client owns the target from here: it strips the spent pairing token
      // and pins the host key the handshake authenticated.
      set({ pairStep: "connecting", pairCode: null });
      host.setState({ connectionError: null });
      goLive(set, get, link);
      try {
        const snapshot = await link.client.connect(target);
        await adopt(set, get, link, snapshot.host);
        host.setState({ workspace: snapshot.workspace });
        if (!snapshot.workspace) await host.getState().refreshWorkspace();
      } catch (e) {
        host.setState({ connectionError: message(e) });
        throw e;
      } finally {
        // Cleared here and nowhere else: everything the user is waiting for has
        // either happened or failed, which is not true at any earlier point the
        // client reports.
        set({ pairStep: null, pairCode: null });
      }
    },

    pairFromLink(target) {
      // The same link can arrive twice — read once from the plugin as the URL
      // the app was launched with, and delivered again by the event it also
      // emits. A second `connect` would tear down the attempt in flight and
      // spend a code that is single use, so the one already running wins.
      // Compared on the whole link, not just the code: a link may carry none,
      // and two of those are not the same pairing.
      const running = get().pairTarget;
      const same =
        running?.pairingToken === target.pairingToken &&
        running?.host === target.host &&
        running?.port === target.port;
      if (get().pairStep && same) return;
      // Pairing adds a host rather than replacing one, so there is nothing to
      // ask. Over a paired host the Pair screen comes up to show the link and how
      // far it has got; before `init` there is no telling yet, and it decides.
      set({ pairTarget: target, addingHost: get().hostKey !== null });
      liveHost().setState({ connectionError: null });
      if (!get().ready) {
        // Held until `init` has read what is on disk — see `queuedLink`. The
        // step is set anyway: the screen is already showing this link, and a
        // button offering to start a second pairing is the same race by hand.
        queuedLink = target;
        set({ pairStep: "connecting" });
        return;
      }
      void get().connect(target).catch(ignore);
    },

    async switchHost(hostKey) {
      const saved = get().hosts.find((h) => h.hostKey === hostKey);
      // Already the live one: there is nothing to drop, and a link that is down
      // has its own Retry.
      if (!saved || (hostKey === get().hostKey && hostLink().client.target?.hostKey === hostKey)) {
        return;
      }
      // Nothing awaits between here and the dial (`connect` drops the link being
      // left first thing): an answer from the host being left that landed in
      // between would read as a new host and be adopted over this one. The
      // clone destination is resolved by `adopt`, once the host has answered.
      // The chosen host is shown from nothing, as if never seen: whatever its
      // store kept from an earlier visit is gone, and so is any PR read it had
      // out. The store being left keeps what it has, off screen.
      const host = hostStoreOf(hostKey);
      host.getState().clear();
      host.setState({ relay: saved.relay ?? null, connectionError: null });
      set({
        hostKey,
        addingHost: false,
        pairTarget: null,
        sheet: null,
        nav: [homeItem()],
      });
      // The user's choice, so the next launch starts here even if this host
      // does not answer now.
      if (!mockEnabled()) void setActiveHost(hostKey);
      await get().connect(targetOf(saved));
    },

    async unpair() {
      const { hostKey } = get();
      if (!hostKey) return;
      // Before the link drops: the host keeps the push token until told
      // otherwise.
      await forgetPush();
      // Its link goes with it, so a later pairing with this host starts afresh
      // — with a store of its own, the one kept by this link going too.
      dropLink(hostKey);
      if (!mockEnabled()) await forgetSavedHost(hostKey);
      const hosts = get().hosts.filter((h) => h.hostKey !== hostKey);
      set({ hosts });
      // Still paired elsewhere: carry on there, straight from this host to the
      // next, rather than by way of the Pair screen.
      const next = hosts[0];
      if (next) {
        await get().switchHost(next.hostKey).catch(ignore);
        return;
      }
      const left = liveHost();
      left.getState().clear();
      left.setState({ relay: null });
      set({
        hostKey: null,
        sheet: null,
        nav: [homeItem()],
        // The link that paired this host carried a code that is long spent;
        // leaving it on the Pair screen would offer the user a dead retry.
        pairTarget: null,
      });
    },

    addHost() {
      // The live link stops while the Pair screen is up: a pairing replaces it
      // anyway, and its retries would otherwise report errors on a screen about
      // another Mac. Cancelling dials it again.
      hostLink().client.disconnect();
      set({ addingHost: true, pairTarget: null, sheet: null });
      liveHost().setState({ connectionError: null });
    },

    async cancelAddHost() {
      set({ addingHost: false, pairTarget: null });
      liveHost().setState({ connectionError: null });
      const active = activeHostOf(get());
      const { client } = hostLink();
      // Back to the host the Pair screen was opened over, which `addHost` or a
      // pairing attempt has left.
      if (active && !(client.target?.hostKey === active.hostKey && client.state === "connected")) {
        await get().connect(targetOf(active)).catch(ignore);
      }
    },

    setTheme(theme) {
      set({ theme });
      void saveSettings({ theme });
    },
    setActiveFirst(activeFirst) {
      set({ activeFirst });
      void saveSettings({ activeFirst });
    },
    setSystemTheme(systemTheme) {
      set({ systemTheme });
    },
    clearError() {
      set({ lastError: null });
    },

    push(screen, props = {}) {
      const key = Date.now() + Math.random();
      set((s) => ({ nav: [...s.nav, { key, screen, props, phase: "enter" }] }));
      // Two frames so the enter transform is painted before it is released.
      requestAnimationFrame(() =>
        requestAnimationFrame(() =>
          set((s) => ({
            nav: s.nav.map((i) =>
              i.key === key && i.phase === "enter" ? { ...i, phase: "idle" } : i,
            ),
          })),
        ),
      );
    },

    pop() {
      const live = get().nav.filter((i) => i.phase !== "leave");
      if (live.length <= 1) return;
      const top = live[live.length - 1];
      set((s) => ({ nav: s.nav.map((i) => (i.key === top.key ? { ...i, phase: "leave" } : i)) }));
      setTimeout(() => set((s) => ({ nav: s.nav.filter((i) => i.phase !== "leave") })), 470);
    },

    /** A tapped alert (docs/remote-protocol.md, "Push notifications"). The stack
     *  is reset first: waking straight into whatever five screens were open last
     *  session is a maze, and the tap named exactly one place to be. */
    openFromPush(fletch) {
      if (!get().ready) {
        queuedPush = fletch;
        return;
      }
      set({ nav: [homeItem()], sheet: null });
      // An alert from a Mac this phone is not paired with — an old token, or a
      // host it has since forgotten — gets no deep link. Home is the Pair screen
      // when there is no pairing, which is the honest answer.
      const { agentId, hostId } = fletch;
      if (!agentId || !hostId || !get().hosts.some((h) => h.hostKey === hostId)) return;
      // An alert about the PR is answered on the Ship tab; a turn ending or a
      // held prompt, in the chat. The agent is opened on the host that sent it.
      const tab = pushTab(fletch.kind);
      const open = () => hostStoreOf(hostId).getState().openAgent(agentId, tab);
      if (hostId === get().hostKey) {
        open();
        return;
      }
      // Every paired Mac holds the token, so an alert can come from one that is
      // not the live one: switch to it, then open the agent it names.
      void get().switchHost(hostId).then(open, ignore);
    },

    openSheet(name, props = {}) {
      set({ sheet: { name, props, open: true } });
    },
    closeSheet() {
      set((s) => (s.sheet ? { sheet: { ...s.sheet, open: false } } : s));
    },
    closeSheetIf(name) {
      if (get().sheet?.name === name) get().closeSheet();
    },
  };
});

/** Read the app store. */
export function useApp<T>(selector: (s: AppState) => T): T {
  return useStore(appStore, selector);
}

/** The store of the link the app is live on: the one on screen. For code
 *  outside a component; a component reads it through `useHostStore`. */
export const liveHost = (): HostStore => storeOfLink(appStore.getState().liveLink);

export type { RawEvent };
