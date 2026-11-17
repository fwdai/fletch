// The app store: what belongs to the phone itself — the theme, navigation and
// sheets (store/nav), the hosts it is paired with, which one is on screen, and
// a pairing in progress (store/pairing). Everything about a host lives in that
// host's store (store/host), one per link; this one dials every paired host,
// each on its own link, and they stay connected for as long as they are paired.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import type { RawEvent } from "../adapters";
import { dropLink, type HostLink, linkOf } from "../host";
import { useHost } from "../host/useHost";
import { ignore } from "../lib/ignore";
import { forgetModels } from "../lib/models";
import { DEFAULT_PORT, type HostTarget, mockEnabled } from "../remote";
import type { PushFletch } from "../remote/push";
import { hostStoreOf, storeOfLink, useHostStore } from "./host";
import { createNavSlice, homeItem, type NavSlice } from "./nav";
import {
  callOffPairing,
  createPairingSlice,
  type PairingSlice,
  pairingInFlight,
  takeQueuedLink,
  watch,
} from "./pairing";
import {
  forgetHost as forgetSavedHost,
  loadSettings,
  type SavedHost,
  saveSettings,
  setActiveHost,
} from "./persist";
import { forgetPush, pushTab, startPush } from "./push";

export {
  type AgentSource,
  agentOf,
  type HostState,
  type HostStore,
  hostStoreOf,
  projectOf,
  RESUME_PROBE_TIMEOUT_MS,
  type SpawnInput,
  storeOfLink,
  useHostStore,
} from "./host";
export { useHostNav } from "./hostNav";
export type { AgentTab, NavItem, ScreenName, SheetName, SheetState } from "./nav";

export type ThemeMode = "system" | "light" | "dark";

export interface AppState extends NavSlice, PairingSlice {
  ready: boolean;
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
  /** The last failed action, on whichever host it ran: one banner for all. */
  lastError: string | null;

  init(): Promise<void>;
  /** Put a paired host on screen, back on Home. Nothing is dropped and nothing
   *  redialled — every paired host is connected already — except a host that
   *  is down with no retry coming, which is dialled again. */
  switchHost(hostKey: string): Promise<void>;
  /** Forget a paired host. It is told to drop the push token first, over its
   *  own link; if it was on screen, the next paired host takes its place, else
   *  the Pair screen. */
  unpair(hostKey: string): Promise<void>;
  setTheme(theme: ThemeMode): void;
  setSystemTheme(t: "light" | "dark"): void;
  setActiveFirst(on: boolean): void;
  clearError(): void;
  openFromPush(fletch: PushFletch): void;
}

let initialized = false;

/** A tapped alert that arrived before `init` finished. It cannot be acted on
 *  yet: which screen it opens depends on the paired hosts, which are still
 *  being read off disk. */
let queuedPush: PushFletch | null = null;

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

/** Connect a paired host's link, greeting it with `hello` — this device is
 *  already registered there — if it is down with no retry coming: never
 *  dialled, or refused. One that is up, on its way up or waiting on its own
 *  retry is left to it. Its store shows how far the dial has got, and how it
 *  failed through the client's own report — which a dial superseded by a newer
 *  attempt on the link (a pairing link naming this host) never makes, so the
 *  newer one's outcome is what shows. From then on the client keeps the link
 *  up on its own, retrying with backoff for as long as the host is paired. */
async function dial(set: Set, get: Get, saved: SavedHost): Promise<void> {
  const link = linkOf(saved.hostKey);
  const { state, retrying } = link.client;
  if (!(state === "disconnected" || (state === "error" && !retrying))) return;
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

export const appStore = createStore<AppState>()((set, get) => {
  return {
    ready: false,
    hosts: [],
    hostKey: null,
    addingHost: false,

    theme: "dark",
    systemTheme: "dark",
    activeFirst: true,
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
          // on, each refreshing what it missed (store/host).
          const links = new Set<HostLink>(get().hosts.map((h) => linkOf(h.hostKey)));
          const pairing = pairingInFlight();
          if (pairing) links.add(pairing);
          for (const link of links) void storeOfLink(link).getState().wake().catch(ignore);
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
      const link = takeQueuedLink();
      // Every paired host at once, each on its own link: one that does not
      // answer holds up none of the others, and its store says why. A host the
      // queued link names is left to the link — see `queuedLink` in
      // store/pairing. None of it is awaited here: the app is usable on
      // whichever hosts answer first.
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
      // Connected all along unless it is down with no retry coming, which is
      // dialled again.
      await dial(set, get, saved);
    },

    async unpair(forgotten) {
      // Over its own link, while it is still up: the host keeps the push token
      // until told otherwise.
      await forgetPush(linkOf(forgotten));
      if (!mockEnabled()) await forgetSavedHost(forgotten);
      // Its model catalog goes with it: the lists are that host's CLIs.
      forgetModels(forgotten);
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

    ...createNavSlice(set, get),

    ...createPairingSlice(set, get),
  };
});

/** Read the app store. */
export function useApp<T>(selector: (s: AppState) => T): T {
  return useStore(appStore, selector);
}

export type { RawEvent };
