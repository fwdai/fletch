// Pairing: the Pair screen's flow, from the target it is handed — typed, picked
// nearby, or a `fletch://pair` link — to the host it adds to the paired ones.
// Also what the app attaches to every link it dials (`watch`), since a pairing
// can land through a link's own retry as well as through `connect`.

import { type HostLink, linkOf, pairingLink } from "../host";
import { ignore } from "../lib/ignore";
import { type HostInfo, type HostTarget, mockEnabled, type PairStep } from "../remote";
import { storeOfLink } from "./host";
import { type AppState, dial } from "./index";
import { homeItem } from "./nav";
import { loadDestParent, saveHost, updateHost, upsertHost } from "./persist";
import { syncPush } from "./push";

type Set = (partial: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => void;
type Get = () => AppState;

export interface PairingSlice {
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
  /** Pair with the host `target` names, on that host's link, or on a link of
   *  its own for an address typed by hand: the host that answers is added to
   *  the paired hosts (or updated there) and put on screen. The hosts already
   *  paired stay connected throughout. */
  connect(target: HostTarget): Promise<void>;
  /** Pair from a `fletch://pair` deep link: show it on the Pair screen and
   *  connect. The host it names is added (or updated, if already paired) and
   *  put on screen. Ignores a repeat of the link already being paired. */
  pairFromLink(target: HostTarget): void;
  /** Open the Pair screen to add another host. */
  addHost(): void;
  /** Leave the Pair screen without adding one, back on the host on screen. */
  cancelAddHost(): void;
}

/** The links the app watches (`watch`). Each is watched once, however often it
 *  is dialled: a second registration would act on every handshake twice. */
const watched = new WeakSet<HostLink>();

/** A pairing link that arrived before `init` finished — the usual case, in
 *  fact: `registerDeepLinks` and `init` start together, and the link has only
 *  two plugin calls to wait on where `init` has the push registration and the
 *  settings read.
 *
 *  It cannot be acted on yet, for two reasons. `init` dials the paired
 *  hosts, and dialling the one the link names on the link they share would
 *  tear this pairing down mid-flight and spend a code that is single use; and
 *  it publishes the settings it read before that, which on a fresh install
 *  means a null host key written over the one a pairing that got in first had
 *  just pinned. */
let queuedLink: HostTarget | null = null;

/** The link of the pairing the Pair screen shows: the one `connect` last
 *  dialled, until it lands (`adopt`) or is called off (`callOffPairing`). A
 *  failed one stays, in case its client is still retrying — unless it is a
 *  paired host's link, which is put back on its saved record instead. */
let pairing: HostLink | null = null;

/** How many `connect`s have started: one that a later one replaced compares
 *  its own number with this, and leaves the Pair screen to the later one. */
let attempts = 0;

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** The pairing link held for `init` (`queuedLink`), handed over once. */
export function takeQueuedLink(): HostTarget | null {
  const link = queuedLink;
  queuedLink = null;
  return link;
}

/** The link of the pairing the Pair screen shows, if one is held. */
export const pairingInFlight = (): HostLink | null => pairing;

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
export function watch(set: Set, get: Get, link: HostLink): void {
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

/** The saved record of the paired host `link` belongs to, if it is one's. */
const savedOf = (get: Get, link: HostLink) => get().hosts.find((h) => h.hostKey === link.hostKey);

/** Stop the pairing the Pair screen shows if it is still running. A paired
 *  host's own link is not stopped but put back on its saved record, dialled
 *  again from it whatever it is doing: the pairing left its own address and
 *  code on the client, which every later reconnect would send again, and a
 *  pairing still running there would otherwise land and take over the screen. */
export function callOffPairing(set: Set, get: Get): void {
  const link = pairing;
  pairing = null;
  if (!link) return;
  const saved = savedOf(get, link);
  if (saved) void dial(set, get, saved, true).catch(ignore);
  else link.client.disconnect();
}

export function createPairingSlice(set: Set, get: Get): PairingSlice {
  return {
    pairStep: null,
    pairCode: null,
    pairTarget: null,
    pairError: null,

    async connect(target) {
      // A host the target names has its own link; an address typed by hand gets
      // a fresh one, which becomes that host's link once it answers — and its
      // store that host's store.
      const link = target.hostKey ? linkOf(target.hostKey) : pairingLink();
      // One pairing at a time. The one this replaces is stopped: its client may
      // still be retrying, and a retry that landed later would take its host's
      // link over from this one's (`pairingLink`) or put it on screen.
      if (pairing !== link) callOffPairing(set, get);
      pairing = link;
      const attempt = ++attempts;
      const current = () => attempt === attempts;
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
        // Only while this pairing still has the Pair screen: one called off
        // (`callOffPairing`) or replaced has nothing more to tell it.
        if (current() && link === pairing) {
          set({ pairError: message(e) });
          // A paired host is not left on a failed pairing's dead code: it is
          // put back on its saved record, and connects again with no tap.
          if (savedOf(get, link)) callOffPairing(set, get);
        }
        throw e;
      } finally {
        // Cleared here and nowhere else: everything the user is waiting for has
        // either happened or failed, which is not true at any earlier point the
        // client reports. Unless a later pairing has the screen by now, whose
        // progress this one's ending says nothing about.
        if (current()) set({ pairStep: null, pairCode: null });
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

    addHost() {
      // The hosts already paired stay connected while the Pair screen is up,
      // and nothing of theirs shows on it; a pairing gets its link when it
      // starts (`connect`).
      set({ addingHost: true, pairTarget: null, pairError: null, sheet: null });
    },

    cancelAddHost() {
      // Back to the host the Pair screen was opened over, connected all along.
      callOffPairing(set, get);
      set({ addingHost: false, pairTarget: null, pairError: null });
    },
  };
}
