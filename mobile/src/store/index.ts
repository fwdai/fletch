// The app store: what belongs to the phone itself — the theme, navigation and
// sheets, the hosts it is paired with, which one is on screen, and a pairing in
// progress. Everything about a host lives in that host's store (store/host),
// one per link; this one dials every paired host, each on its own link, and
// they stay connected for as long as they are paired.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import type { RawEvent } from "../adapters";
import { dropLink, type HostLink, linkOf, pairingLink } from "../host";
import { useHost } from "../host/useHost";
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
  hostStoreOf,
  refreshChats,
  refreshOpenAgent,
  refreshProviders,
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
  type HostState,
  type HostStore,
  hostStoreOf,
  projectOf,
  type SpawnInput,
  storeOfLink,
  useHostStore,
} from "./host";
export { useHostNav } from "./hostNav";

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
  /** What the screen shows. `hostKey` names the host it acts on (`withHost`
   *  in store/hostNav); Home, which names none, acts on the host on screen. */
  props: Record<string, string>;
  phase: "enter" | "idle" | "leave";
}

/** The agent screen's tabs, as its `tab` nav prop names them. */
export type AgentTab = "chat" | "changes" | "ship";

export interface SheetState {
  name: SheetName;
  /** As a screen's: `hostKey` names the host the sheet acts on. */
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
  /** Why the last pairing failed, for the Pair screen. Cleared when a pairing
   *  starts, and on the way into or out of the Pair screen (`addHost`,
   *  `cancelAddHost`, `switchHost`). */
  pairError: string | null;
  /** Every host this phone is paired with (store/persist), each connected on
   *  its own link. One is on screen — `hostKey` — and the Home header switches
   *  between them. */
  hosts: SavedHost[];
  /** The public key of the host on screen — pinned on first contact, and what
   *  makes the app paired at all. */
  hostKey: string | null;
  /** The Pair screen is up to add another host over a paired one, rather than
   *  because there is none. Cleared by a pairing that lands, or by cancelling
   *  back to the host on screen. */
  addingHost: boolean;

  theme: ThemeMode;
  systemTheme: "light" | "dark";
  /** Home lists projects with recent agent activity first (see
   *  `lib/activeProjects`). Off keeps the host's order. */
  activeFirst: boolean;
  nav: NavItem[];
  sheet: SheetState | null;
  /** The last failed action, on whichever host it ran: one banner for all. */
  lastError: string | null;

  init(): Promise<void>;
  /** Pair with the host `target` names, on that host's link, or on a link of
   *  its own for an address typed by hand: the host that answers is added to
   *  the paired hosts (or updated there) and put on screen. The hosts already
   *  paired stay connected throughout. */
  connect(target: HostTarget): Promise<void>;
  /** Pair from a `fletch://pair` deep link: show it on the Pair screen and
   *  connect. The host it names is added (or updated, if already paired) and
   *  put on screen. Ignores a repeat of the link already being paired. */
  pairFromLink(target: HostTarget): void;
  /** Put a paired host on screen, back on Home. Nothing is dropped and nothing
   *  redialled — every paired host is connected already — except a host that
   *  is down with no retry coming, which is dialled again. */
  switchHost(hostKey: string): Promise<void>;
  /** Forget a paired host. It is told to drop the push token first, over its
   *  own link; if it was on screen, the next paired host takes its place, else
   *  the Pair screen. */
  unpair(hostKey: string): Promise<void>;
  /** Open the Pair screen to add another host. */
  addHost(): void;
  /** Leave the Pair screen without adding one, back on the host on screen. */
  cancelAddHost(): void;
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
}

let initialized = false;

/** The links the app watches (`watch`). Each is watched once, however often it
 *  is dialled: a second registration would act on every handshake twice. */
const watched = new WeakSet<HostLink>();

/** How long a foregrounded app waits for a host to answer the workspace read
 *  before treating the socket as dead and reconnecting. `get_workspace` is one
 *  database read and answers in well under a second even over the relay, so a
 *  miss this long is the socket, not the host being busy. */
export const RESUME_PROBE_TIMEOUT_MS = 6_000;

/** A tapped alert that arrived before `init` finished. It cannot be acted on
 *  yet: which screen it opens depends on the paired hosts, which are still
 *  being read off disk. */
let queuedPush: PushFletch | null = null;

/** A pairing link that arrived before `init` finished — the usual case, in
 *  fact: `registerDeepLinks` and `init` start together, and the link has only
 *  two plugin calls to wait on where `init` has the push registration and the
 *  settings read.
 *
 *  It cannot be acted on yet either, for two reasons. `init` dials the paired
 *  hosts, and dialling the one the link names on the link they share would
 *  tear this pairing down mid-flight and spend a code that is single use; and
 *  it publishes the settings it read before that, which on a fresh install
 *  means a null host key written over the one a pairing that got in first had
 *  just pinned. */
let queuedLink: HostTarget | null = null;

/** The link of the pairing the Pair screen shows: the one `connect` last
 *  dialled, until it lands (`adopt`) or is called off (`callOffPairing`). A
 *  failed one stays, in case its client is still retrying. */
let pairing: HostLink | null = null;

const homeItem = (): NavItem => ({ key: Date.now(), screen: "home", props: {}, phase: "idle" });

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** What to call the host this component acts on (`useHost`). The saved name
 *  first: the client's idea of the host lags a pairing until it has answered,
 *  and the saved one is kept current by every handshake. */
export function useHostName(): string | null {
  const { hostKey } = useHost();
  const saved = useApp((s) => s.hosts.find((h) => h.hostKey === hostKey)?.name);
  const answered = useHostStore((s) => s.hostInfo?.name);
  return saved ?? answered ?? null;
}

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

type Set = (partial: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => void;
type Get = () => AppState;

/** Everything that makes the app paired with the host it has just greeted: the
 *  pinned key, the path the link came in on, and the durable record of both —
 *  added to the paired hosts, or updated there, and put on screen.
 *
 *  Shared by `connect` and `watch` because a first connection can complete
 *  through either. A pairing that is answered and then loses the socket before
 *  the workspace arrives has already spent its code, and the client retries it
 *  on its own with the key it pinned — that retry has no `connect` above it,
 *  and without this the app would sit on the Pair screen holding a working
 *  connection. */
async function adopt(set: Set, get: Get, link: HostLink, host: HostInfo): Promise<void> {
  // Landed: the link is its host's from here, and the pairing is over.
  if (link === pairing) pairing = null;
  const { client } = link;
  // The client owns the target: the spent pairing token is gone from it and
  // the host key the handshake authenticated is pinned in.
  const target = client.target;
  const hostKey = client.hostKey ?? get().hostKey;
  const relay = target?.relay ?? null;
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
  storeOfLink(link).setState({ lastDestParent });
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

/** What `link`'s pairing steps and handshakes mean for the app, whichever host
 *  is on screen: the Pair screen's progress while it is the link pairing, the
 *  host adopted when a pairing lands, a paired host's saved name and relay kept
 *  current, and the push token handed over. Everything about the host itself
 *  is its store's, handshakes included (store/host). */
function watch(set: Set, get: Get, link: HostLink): void {
  if (watched.has(link)) return;
  watched.add(link);
  const { client } = link;
  // Only while a `connect` owns the flow on this link: a background reconnect
  // is the connection banner's business, and moving the Pair screen's progress
  // on behalf of one would be describing a connection the user did not ask
  // for.
  const isPairing = () => link === pairing && get().pairStep !== null;
  client.onStep((step) => {
    if (isPairing()) set({ pairStep: step });
  });
  client.onConfirmCode((code) => {
    if (isPairing()) set({ pairCode: code });
  });
  client.onSnapshot((snapshot) => {
    const saved = get().hosts.find((h) => h.hostKey === client.hostKey);
    if (saved) {
      // The client has folded the host's relay into its target; a reconnect
      // keeps it on the saved entry — and the name the host answers to now,
      // for the switcher and the header.
      const patch = { relay: client.target?.relay, name: snapshot.host.name || saved.name };
      if (patch.relay !== saved.relay || patch.name !== saved.name) {
        set((s) => ({
          hosts: s.hosts.map((h) => (h.hostKey === saved.hostKey ? { ...h, ...patch } : h)),
        }));
        if (!mockEnabled()) void updateHost(saved.hostKey, patch);
      }
    } else if (!get().pairStep) {
      // A host not paired yet. `connect` adopts the one it pairs itself, so
      // this is for the retry that completes a pairing whose first attempt
      // died after the code was spent. Guarded on `pairStep` so the two never
      // race.
      void adopt(set, get, link, snapshot.host).catch(ignore);
    }
    // Every handshake — the first pairing and every reconnect — is when the
    // host is told the APNs token again.
    void syncPush(link).catch(ignore);
  });
}

/** Connect a paired host's link, greeting it with `hello` — this device is
 *  already registered there. Its store shows how far the dial has got, and
 *  how it failed through the client's own report — which a dial superseded by
 *  a newer attempt on the link (a pairing link naming this host) never makes,
 *  so the newer one's outcome is what shows. From then on the client keeps the
 *  link up on its own, retrying with backoff for as long as the host is
 *  paired. */
async function dial(set: Set, get: Get, saved: SavedHost): Promise<void> {
  const link = linkOf(saved.hostKey);
  const host = storeOfLink(link);
  watch(set, get, link);
  host.setState({ connectionError: null, dialStep: "connecting" });
  const off = link.client.onStep((dialStep) => host.setState({ dialStep }));
  try {
    await link.client.connect(targetOf(saved));
  } finally {
    off();
    host.setState({ dialStep: null });
  }
}

/** Stop the pairing the Pair screen shows if it is still running — unless its
 *  link is a paired host's, which stays connected like every other. */
function callOffPairing(get: Get): void {
  const link = pairing;
  pairing = null;
  if (link && !get().hosts.some((h) => h.hostKey === link.hostKey)) link.client.disconnect();
}

/** Back in the foreground: refresh what the background missed on `link`'s
 *  host — once its socket has shown it is still alive. iOS freezes a suspended
 *  app's sockets, and when the app returns one can read as connected for a
 *  while longer with every request on it hanging. So the refresh doubles as
 *  the probe: a workspace read the host does not answer in time, or that the
 *  socket refuses outright, means the socket is dead, and the reconnect it
 *  triggers does the refreshing instead, through the handshake's snapshot. The
 *  transport's own pings catch the same death for an idle foreground app, more
 *  slowly. */
function wake(link: HostLink): void {
  const { client } = link;
  // A retry waiting out its backoff was scheduled while the phone was away;
  // the user looking at the app is reason enough to dial now.
  if (client.retrying) {
    void storeOfLink(link).getState().reconnect().catch(ignore);
    return;
  }
  if (client.state !== "connected") return;
  const store = storeOfLink(link);
  void (async () => {
    const alive = await withTimeout(
      store.getState().refreshWorkspace(),
      RESUME_PROBE_TIMEOUT_MS,
      false,
    );
    const host = store.getState();
    if (!alive) {
      void host.reconnect().catch(ignore);
      return;
    }
    void host.loadShortstats();
    // A `pr:*` event that reached a suspended app may never have been read.
    void host.loadPrStatus();
    refreshOpenAgent(link.hostKey, host);
    refreshProviders(link.hostKey, host);
    refreshChats(host);
  })();
}

export const appStore = createStore<AppState>()((set, get) => {
  return {
    ready: false,
    pairStep: null,
    pairCode: null,
    pairTarget: null,
    hosts: [],
    pairError: null,
    hostKey: null,
    addingHost: false,

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
      await startPush({ get }).catch(ignore);
      if (typeof document !== "undefined") {
        document.addEventListener("visibilitychange", () => {
          if (document.hidden) return;
          // Every paired host, and a pairing the Pair screen is still waiting
          // on.
          const links = new Set<HostLink>(get().hosts.map((h) => linkOf(h.hostKey)));
          if (pairing) links.add(pairing);
          for (const link of links) wake(link);
        });
      }
      const saved = await loadSettings();
      // Mock mode keeps to its one in-memory host: real hosts saved by an
      // earlier run would only dial the mock under their names.
      const hosts = mockEnabled() ? [] : (saved.hosts ?? []);
      const active = hosts.find((h) => h.hostKey === saved.activeHostKey) ?? hosts[0] ?? null;
      const hostKey = active?.hostKey ?? null;
      // The one thing about a host its store cannot learn from the host: where
      // its last clone landed, which only the disk remembers.
      for (const h of hosts) {
        hostStoreOf(h.hostKey).setState({ lastDestParent: saved.destParents?.[h.hostKey] ?? null });
      }
      set({
        ready: true,
        theme: saved.theme ?? "dark",
        activeFirst: saved.activeFirst ?? true,
        hosts,
        hostKey,
      });
      const link = queuedLink;
      queuedLink = null;
      // Every paired host at once, each on its own link: one that does not
      // answer holds up none of the others, and its store says why. A host the
      // queued link names is left to the link — see `queuedLink`. None of it is
      // awaited here: the app is usable on whichever hosts answer first.
      const dials = hosts.filter((h) => h.hostKey !== link?.hostKey).map((h) => dial(set, get, h));
      // After the dials have started, so a tap from a host not on screen finds
      // it connecting rather than dialling it a second time; and without
      // waiting on any of them — its agent opens at once, and is read again
      // when its host answers.
      if (queuedPush) {
        const fletch = queuedPush;
        queuedPush = null;
        get().openFromPush(fletch);
      }
      if (link) {
        // A scanned link says to pair with *this* Mac. Over a paired host the
        // Pair screen shows it as an addition, with a way back to that host if
        // it fails.
        set({ addingHost: hostKey !== null });
        await get().connect(link).catch(ignore);
      } else if (mockEnabled()) {
        // The mock host has no pairing step worth clicking through every reload;
        // its fixed key is pinned by `connect` like any other.
        await get().connect({ host: "mock", port: DEFAULT_PORT }).catch(ignore);
      }
      await Promise.allSettled(dials);
    },

    async connect(target) {
      // A host the target names has its own link; an address typed by hand gets
      // a fresh one, which becomes that host's link once it answers — and its
      // store that host's store.
      const link = target.hostKey ? linkOf(target.hostKey) : pairingLink();
      pairing = link;
      // The client owns the target from here: it strips the spent pairing token
      // and pins the host key the handshake authenticated.
      set({ pairStep: "connecting", pairCode: null, pairError: null });
      // Its store and the app's watch, both before it dials, so nothing it
      // says in between is missed — the handshake included, which the store
      // takes (store/host).
      storeOfLink(link);
      watch(set, get, link);
      try {
        const snapshot = await link.client.connect(target);
        await adopt(set, get, link, snapshot.host);
      } catch (e) {
        set({ pairError: message(e) });
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
      // Already on screen: there is nothing to do, and a host that is down has
      // its own Retry.
      if (!saved || (hostKey === get().hostKey && !get().addingHost)) return;
      // Leaving the Pair screen for it: a pairing still running would land on
      // screen over the host chosen here.
      if (get().addingHost) callOffPairing(get);
      set({
        hostKey,
        addingHost: false,
        pairTarget: null,
        pairError: null,
        sheet: null,
        nav: [homeItem()],
      });
      // The user's choice, so the next launch starts here even if this host
      // does not answer now.
      if (!mockEnabled()) void setActiveHost(hostKey);
      const { state, retrying } = linkOf(hostKey).client;
      if (state === "connected") {
        // Kept current off screen by its events, all but the working-tree stats,
        // which only the on-screen host's poll reads (App).
        void hostStoreOf(hostKey).getState().loadShortstats();
        return;
      }
      // Down with no retry coming — never dialled, or refused — so it is
      // dialled again. One retrying is left to its retry, and one connecting to
      // its attempt.
      if (state === "disconnected" || (state === "error" && !retrying)) await dial(set, get, saved);
    },

    async unpair(forgotten) {
      // Over its own link, while it is still up: the host keeps the push token
      // until told otherwise.
      await forgetPush(linkOf(forgotten));
      if (!mockEnabled()) await forgetSavedHost(forgotten);
      const hosts = get().hosts.filter((h) => h.hostKey !== forgotten);
      // Nothing left on screen names it: whatever does would reach for its link
      // after it has gone.
      set((s) => ({
        hosts,
        nav: s.nav.filter((n) => n.props.hostKey !== forgotten),
        sheet: s.sheet?.props.hostKey === forgotten ? null : s.sheet,
      }));
      let next: Promise<void> | undefined;
      if (forgotten === get().hostKey) {
        if (hosts[0]) {
          // Still paired elsewhere: carry on there, straight from this host to
          // the next, rather than by way of the Pair screen.
          next = get().switchHost(hosts[0].hostKey).catch(ignore);
        } else {
          set({
            hostKey: null,
            sheet: null,
            nav: [homeItem()],
            // The link that paired this host carried a code that is long spent;
            // leaving it on the Pair screen would offer the user a dead retry.
            pairTarget: null,
          });
        }
      }
      // Its link goes with it, so a later pairing with this host starts afresh —
      // with a store of its own, the one kept by this link going too. Dropped
      // only once it is off screen.
      dropLink(forgotten);
      await next;
    },

    addHost() {
      // The hosts already paired stay connected while the Pair screen is up,
      // and nothing of theirs shows on it; a pairing gets its link when it
      // starts (`connect`).
      set({ addingHost: true, pairTarget: null, pairError: null, sheet: null });
    },

    cancelAddHost() {
      // Back to the host the Pair screen was opened over, connected all along.
      callOffPairing(get);
      set({ addingHost: false, pairTarget: null, pairError: null });
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
      // An alert from a Mac this phone is not paired with — an old token, or a
      // host it has since forgotten — gets no deep link. Home is the Pair screen
      // when there is no pairing, which is the honest answer.
      const { agentId, hostId } = fletch;
      const paired = get().hosts.some((h) => h.hostKey === hostId);
      // Every paired Mac holds the token, so an alert can come from one that is
      // not on screen: it is put there, connected all along (or dialling, if it
      // was down), and nothing is dropped.
      if (hostId && paired && hostId !== get().hostKey) void get().switchHost(hostId).catch(ignore);
      set({ nav: [homeItem()], sheet: null });
      if (!agentId || !hostId || !paired) return;
      // An alert about the PR is answered on the Ship tab; a turn ending or a
      // held prompt, in the chat. The agent is opened on the host that sent it,
      // at once: a host still connecting re-reads it when it answers.
      hostStoreOf(hostId).getState().openAgent(agentId, pushTab(fletch.kind));
    },

    openSheet(name, props = {}) {
      set({ sheet: { name, props, open: true } });
    },
    closeSheet() {
      set((s) => (s.sheet ? { sheet: { ...s.sheet, open: false } } : s));
    },
  };
});

/** Read the app store. */
export function useApp<T>(selector: (s: AppState) => T): T {
  return useStore(appStore, selector);
}

export type { RawEvent };
