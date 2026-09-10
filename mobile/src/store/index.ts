import type { AgentRecord, Workspace } from "@desktop/api/types/agent";
import type { CheckoutFile, DirListing } from "@desktop/api/types/checkout";
import type { DiffStats, GitState } from "@desktop/api/types/git";
import type { PrChecks, PrState } from "@desktop/api/types/pr";
import type { GhRepoSummary, GhStatus } from "@desktop/api/types/providers";
import { create } from "zustand";
import type { ChatItem, RawEvent } from "../adapters";
import { createApi } from "../api";
import { isBusy } from "../lib/agents";
import { ignore } from "../lib/ignore";
import {
  type ConnectionState,
  createClient,
  DEFAULT_PORT,
  type HostInfo,
  type HostTarget,
  mockEnabled,
  type PairStep,
  type Via,
} from "../remote";
import type { PushFletch } from "../remote/push";
import { registerRemoteEvents } from "./events";
import { clearHost, loadDestParent, loadSettings, saveDestParent, saveSettings } from "./persist";
import { forgetPush, startPush, syncPush } from "./push";
import { applyUserTurns, reduceRecords } from "./transcript";

export const client = createClient();
export const api = createApi(client);

export type ThemeMode = "system" | "light" | "dark";
export type ScreenName = "home" | "project" | "agent" | "file" | "diff";
export type SheetName = "host" | "newAgent" | "addProject" | "agentMore" | "modelPicker" | "pr";

export interface NavItem {
  key: number;
  screen: ScreenName;
  props: Record<string, string>;
  phase: "enter" | "idle" | "leave";
}

export interface SheetState {
  name: SheetName;
  props: Record<string, string>;
  open: boolean;
}

export interface MobileState {
  ready: boolean;
  connection: ConnectionState;
  connectionError: string | null;
  /** How far the `connect` in flight has got, and null when none is.
   *
   *  Deliberately wider than the client's `connected`, which arrives as soon
   *  as `pair` is answered with the workspace still to fetch: a Pair screen
   *  that goes idle there has told the user it gave up while it is in fact
   *  most of the way through. It is also the whole progress display — the
   *  waits are long enough (a LAN dial timing out, then a relay) that a
   *  screen showing nothing reads as a hung app. */
  pairStep: PairStep | null;
  /** The `fletch://pair` link the app was opened with, so the Pair screen can
   *  show which Mac it is pairing with rather than an empty form — and keep
   *  the details for a one-tap retry if it fails. */
  pairTarget: HostTarget | null;
  hostInfo: HostInfo | null;
  /** The paired host's public key — pinned on first contact, and what makes
   *  the app paired at all. */
  hostKey: string | null;
  /** The host's relay base URL, when it has one; the fallback path. */
  relay: string | null;
  /** Which path the live connection took — mirrored from the client for the
   *  Host sheet, and null when there is no connection. */
  via: Via | null;
  /** Where the last clone landed on this host, so the next one is offered the
   *  same folder. Hydrated from the persist layer when the host is known. */
  lastDestParent: string | null;

  workspace: Workspace | null;
  logs: Record<string, ChatItem[]>;
  busy: Record<string, boolean>;
  /** tool_use id → held control-protocol request id, per agent. */
  pendingToolUse: Record<string, Record<string, string>>;
  turnStartedAt: Record<string, number>;
  gitStates: Record<string, GitState | null>;
  diffStats: Record<string, DiffStats>;
  prStates: Record<string, PrState | null>;
  prChecks: Record<string, PrChecks | null>;
  trees: Record<string, CheckoutFile[]>;

  theme: ThemeMode;
  systemTheme: "light" | "dark";
  nav: NavItem[];
  sheet: SheetState | null;
  lastError: string | null;

  init(): Promise<void>;
  connect(target: HostTarget): Promise<void>;
  /** Pair from a `fletch://pair` deep link: show it on the Pair screen and
   *  connect. Ignores a repeat of the link already being paired. */
  pairFromLink(target: HostTarget): void;
  reconnect(): Promise<void>;
  unpair(): Promise<void>;
  /** Add or change the relay for the paired host without re-pairing. */
  setRelay(url: string | null): Promise<void>;
  setTheme(theme: ThemeMode): void;
  setSystemTheme(t: "light" | "dark"): void;
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

  refreshWorkspace(): Promise<void>;
  openAgent(agentId: string): void;
  loadAgent(agentId: string): Promise<void>;
  rebuildLog(agentId: string): Promise<void>;
  loadGit(agentId: string): Promise<void>;
  loadTree(agentId: string): Promise<void>;

  send(agentId: string, text: string): Promise<void>;
  spawn(input: SpawnInput): Promise<void>;
  answerToolUse(
    agentId: string,
    toolUseId: string,
    updatedInput: unknown,
    behavior: "allow" | "deny",
  ): Promise<void>;
  stop(agentId: string): Promise<void>;
  resume(agentId: string): Promise<void>;
  archive(agentId: string): Promise<void>;
  /** `null` clears the pinned model, leaving the provider CLI's own default. */
  setModel(agentId: string, model: string | null): Promise<void>;
  setEffort(agentId: string, effort: string): Promise<void>;
  publish(agentId: string, title: string, body: string): Promise<void>;
  pushToPr(agentId: string): Promise<void>;

  listDir(path: string): Promise<DirListing>;
  addWorkspaceRepo(repoPath: string): Promise<void>;
  cloneRepo(spec: string, destParent: string): Promise<void>;
  ghStatus(): Promise<GhStatus>;
  ghRepoList(): Promise<GhRepoSummary[]>;
}

export interface SpawnInput {
  repoPath: string;
  provider: string;
  model: string | null;
  effort: string | null;
  base: string;
  prompt: string;
  /** The workspace name the New Agent sheet showed. Empty when the sheet
   *  never got one (its allocation failed), in which case one is allocated
   *  here so the spawn still goes through. */
  name: string;
}

let initialized = false;

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

const newId = () =>
  globalThis.crypto?.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36)}`;

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

type Setter = (partial: Partial<MobileState>) => void;

/** Record the failure where the error UI can see it, then rethrow: an action
 *  that swallows leaves its caller believing it succeeded — which is how a
 *  failed spawn used to clear the prompt the user still needs. */
async function guard<T>(set: Setter, fn: () => Promise<T>): Promise<T> {
  try {
    return await fn();
  } catch (e) {
    set({ lastError: message(e) });
    throw e;
  }
}

/** Everything that makes the app paired with the host it has just greeted: the
 *  pinned key, the path the link came in on, and the durable record of both.
 *
 *  Shared by `connect` and the snapshot subscription because a first
 *  connection can complete through either. A pairing that is answered and then
 *  loses the socket before the workspace arrives has already spent its code,
 *  and the client retries it on its own with the key it pinned — that retry
 *  has no `connect` above it, and without this the app would sit on the Pair
 *  screen holding a working connection. */
async function adopt(set: Setter, get: () => MobileState, host: HostInfo): Promise<void> {
  // The client owns the target: the spent pairing token is gone from it and
  // the host key the handshake authenticated is pinned in.
  const target = client.target;
  const hostKey = client.hostKey ?? get().hostKey;
  const relay = target?.relay ?? null;
  set({
    hostInfo: host,
    hostKey,
    relay,
    via: client.via,
    // The remembered clone destination is per host, so it can only be
    // resolved once the handshake says which host this is.
    lastDestParent: await loadDestParent(hostKey),
    nav: [homeItem()],
  });
  // Mock mode must not leave a "mock" host behind for the next real run.
  if (mockEnabled() || !target) return;
  await saveSettings({
    host: target.host,
    port: target.port,
    hostName: host.name,
    relay: relay ?? undefined,
    ...(hostKey ? { hostKey } : {}),
  });
}

export const agentOf = (ws: Workspace | null, id: string): AgentRecord | undefined =>
  ws?.agents.find((a) => a.id === id);

export const projectOf = (ws: Workspace | null, agentId: string) => {
  const agent = agentOf(ws, agentId);
  return ws?.projects.find((p) => p.project_id === agent?.project_id);
};

/** Wait for a freshly spawned agent to leave `spawning` before the first
 *  message is sent — the spawn flow in docs/remote-protocol.md. */
function waitForSpawn(get: () => MobileState, agentId: string, timeoutMs = 30_000) {
  return new Promise<void>((resolve, reject) => {
    const started = Date.now();
    const tick = () => {
      const status = agentOf(get().workspace, agentId)?.status;
      if (status && status !== "spawning") return resolve();
      if (Date.now() - started > timeoutMs) return reject(new Error("agent never started"));
      setTimeout(tick, 150);
    };
    tick();
  });
}

export const useStore = create<MobileState>()((set, get) => ({
  ready: false,
  connection: "disconnected",
  connectionError: null,
  pairStep: null,
  pairTarget: null,
  hostInfo: null,
  hostKey: null,
  relay: null,
  via: null,
  lastDestParent: null,

  workspace: null,
  logs: {},
  busy: {},
  pendingToolUse: {},
  turnStartedAt: {},
  gitStates: {},
  diffStats: {},
  prStates: {},
  prChecks: {},
  trees: {},

  theme: "dark",
  systemTheme: "dark",
  nav: [{ key: 1, screen: "home", props: {}, phase: "idle" }],
  sheet: null,
  lastError: null,

  async init() {
    // Idempotent: React StrictMode mounts effects twice in dev, and a second
    // registration would fold every host event into the log twice.
    if (initialized) return;
    initialized = true;
    registerRemoteEvents(client, set, get);
    // Early, and awaited: the plugin holds the APNs token — and a tap that
    // launched the app — until it is asked to register, and hands both over
    // through events these listeners have to be in place for.
    await startPush({ client, api, get }).catch(ignore);
    client.onState((state, error) =>
      set({
        connection: state,
        connectionError: error ?? null,
        hostInfo: client.host,
        via: client.via,
      }),
    );
    // Only while a `connect` owns the flow: a background reconnect is the
    // connection banner's business, and moving the Pair screen's progress on
    // behalf of one would be describing a connection the user did not ask for.
    client.onStep((step) => {
      if (get().pairStep) set({ pairStep: step });
    });
    // The log of an open thread is kept current by live events, which a
    // dropped socket or a backgrounded webview silently misses — so every
    // reconnect and every return to the foreground re-reads it from the host.
    const refreshOpenAgent = () => {
      const agentId = [...get().nav].reverse().find((n) => n.props.agentId)?.props.agentId;
      if (agentId) void get().loadAgent(agentId).catch(ignore);
    };
    client.onSnapshot((snapshot) => {
      set({ hostInfo: snapshot.host });
      if (snapshot.workspace) set({ workspace: snapshot.workspace });
      else void get().refreshWorkspace();
      refreshOpenAgent();
      // Every handshake — the first pairing and every reconnect — is when the
      // host is told the APNs token again.
      void syncPush().catch(ignore);
      // A handshake that lands while the app still has no host key is the one
      // that pairs it: `connect` does this itself, so this is for the retry
      // that completes a pairing whose first attempt died after the code was
      // spent. Guarded on `pairStep` so the two never race.
      if (!get().hostKey && !get().pairStep) void adopt(set, get, snapshot.host).catch(ignore);
    });
    if (typeof document !== "undefined") {
      document.addEventListener("visibilitychange", () => {
        if (document.hidden || client.state !== "connected") return;
        void get().refreshWorkspace();
        refreshOpenAgent();
      });
    }
    const saved = await loadSettings();
    // The pinned key comes back before `ready`, because it is what a queued
    // notification tap has to be checked against — a tap must not wait out a
    // connection attempt to open its agent.
    const hostKey = saved.host && saved.hostKey ? saved.hostKey : null;
    set({ ready: true, theme: saved.theme ?? "dark", relay: saved.relay ?? null, hostKey });
    if (queuedPush) {
      const fletch = queuedPush;
      queuedPush = null;
      get().openFromPush(fletch);
    }
    // Before the saved host, and instead of it: a scanned link says to pair
    // with *this* Mac, which is an instruction, where reconnecting the last
    // one is only a default. Racing them would supersede the pairing and burn
    // its code.
    if (queuedLink) {
      const target = queuedLink;
      queuedLink = null;
      await get().connect(target).catch(ignore);
      return;
    }
    if (mockEnabled()) {
      // The mock host has no pairing step worth clicking through every reload;
      // its fixed key is pinned by `connect` like any other.
      await get().connect({ host: "mock", port: DEFAULT_PORT }).catch(ignore);
      return;
    }
    // A saved host key is the whole credential: `hello` authenticates with the
    // device key the Rust layer holds. Stood down while a pairing is running,
    // for the same reason the queued link goes first: a link that arrived in
    // the moment between `ready` and here is already connecting, and this
    // would replace it.
    if (saved.host && hostKey && !get().pairStep) {
      set({ lastDestParent: saved.destParents?.[hostKey] ?? null });
      await get()
        .connect({
          host: saved.host,
          port: saved.port ?? DEFAULT_PORT,
          name: saved.hostName,
          hostKey,
          // The relay is dialled only if the LAN address does not answer.
          relay: saved.relay,
        })
        .catch(ignore);
    }
  },

  async connect(target) {
    // The client owns the target from here: it strips the spent pairing token
    // and pins the host key the handshake authenticated.
    set({ connectionError: null, pairStep: "connecting" });
    try {
      const snapshot = await client.connect(target);
      await adopt(set, get, snapshot.host);
      set({ workspace: snapshot.workspace });
      if (!snapshot.workspace) await get().refreshWorkspace();
    } catch (e) {
      set({ connectionError: message(e) });
      throw e;
    } finally {
      // Cleared here and nowhere else: everything the user is waiting for has
      // either happened or failed, which is not true at any earlier point the
      // client reports.
      set({ pairStep: null });
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
    set({ pairTarget: target, connectionError: null });
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

  /** Retry the link the client already holds — the store no longer keeps a
   *  copy of the target, so there is no stale pairing token to replay. */
  async reconnect() {
    if (!client.target) return;
    set({ connectionError: null });
    // The snapshot subscription in `init` folds the fresh host and workspace
    // in; the state subscription reports the failure.
    await client.reconnect();
  },

  async unpair() {
    // Before the link drops: the host keeps the push token until told otherwise.
    await forgetPush();
    client.disconnect();
    await clearHost();
    set({
      hostKey: null,
      relay: null,
      via: null,
      lastDestParent: null,
      workspace: null,
      hostInfo: null,
      logs: {},
      sheet: null,
      nav: [homeItem()],
      // The link that paired this host carried a code that is long spent;
      // leaving it on the Pair screen would offer the user a dead retry.
      pairTarget: null,
    });
  },

  /** The relay is a property of the paired host, not of a pairing: a link that
   *  never carried one (or a hand-typed pairing) can be given one here, and it
   *  applies from the next connection attempt on. */
  async setRelay(url) {
    const relay = url?.trim() || null;
    client.setRelay(relay);
    set({ relay });
    if (!mockEnabled()) await saveSettings({ relay: relay ?? undefined });
  },

  setTheme(theme) {
    set({ theme });
    void saveSettings({ theme });
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
    const { hostKey } = get();
    if (!fletch.agentId || !hostKey || fletch.hostId !== hostKey) return;
    get().openAgent(fletch.agentId);
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

  async refreshWorkspace() {
    try {
      const workspace = await api.getWorkspace();
      if (workspace) set({ workspace });
    } catch {
      // Best effort; the next event or resync recovers.
    }
  },

  openAgent(agentId) {
    get().push("agent", { agentId });
    void get().loadAgent(agentId).catch(ignore);
  },

  async loadAgent(agentId) {
    // A running turn's log is built from live events; the host's records only
    // catch up at turn end (see rebuildLog). Rebuilding now would replace it
    // with the prompt alone, so a re-open or reconnect mid-turn keeps what the
    // stream rendered. The turn-end `session:records-appended` rebuild — which
    // calls rebuildLog directly — is the authoritative one.
    const agent = agentOf(get().workspace, agentId);
    const midTurn = agent !== undefined && isBusy(agent) && get().logs[agentId] !== undefined;
    if (!midTurn) await get().rebuildLog(agentId);
    await get().loadGit(agentId);
  },

  async rebuildLog(agentId) {
    return guard(set, async () => {
      let [records, turns] = await Promise.all([
        api.readSessionRecords(agentId),
        api.readUserTurns(agentId),
      ]);
      // The host ingests a turn's transcript into session_records only at
      // turn-end, and that ingest can insert nothing (session id not captured
      // yet, transcript not located). Mirror the desktop's readReducedLog: ask
      // for a backfill and read again before concluding there is no history.
      if (records.length === 0) {
        await api.syncSession(agentId);
        [records, turns] = await Promise.all([
          api.readSessionRecords(agentId),
          api.readUserTurns(agentId),
        ]);
      }
      // Still nothing stored: keep the log the live events built rather than
      // wiping the conversation the user was just looking at (the desktop's
      // records-appended handler makes the same call). A brand-new agent has
      // no log either way, so the empty state still renders for it.
      if (records.length === 0) return;
      const provider = agentOf(get().workspace, agentId)?.provider;
      const items = applyUserTurns(reduceRecords(provider, records), turns);
      set((s) => ({ logs: { ...s.logs, [agentId]: items } }));
    });
  },

  async loadGit(agentId) {
    try {
      const [git, diff, pr] = await Promise.all([
        api.getGitState(agentId),
        api.getAgentDiffStats(agentId),
        api.getPrState(agentId),
      ]);
      set((s) => ({
        gitStates: { ...s.gitStates, [agentId]: git },
        diffStats: { ...s.diffStats, [agentId]: diff },
        prStates: { ...s.prStates, [agentId]: pr },
      }));
      if (pr) {
        const checks = await api.getPrChecks(agentId);
        set((s) => ({ prChecks: { ...s.prChecks, [agentId]: checks } }));
      }
    } catch {
      // Git/PR reads are advisory; leave the last-known values alone.
    }
  },

  async loadTree(agentId) {
    return guard(set, async () => {
      const tree = await api.listCheckoutTree(agentId);
      set((s) => ({ trees: { ...s.trees, [agentId]: tree } }));
    });
  },

  async send(agentId, text) {
    const trimmed = text.trim();
    if (!trimmed) return;
    // Optimistic bubble, reconciled away when the canonical records land. It
    // carries the turn id so the host's `turn:sent` echo (which every device
    // mirrors, see events.ts) is recognized as ours and not drawn twice.
    const turnId = newId();
    set((s) => ({
      logs: {
        ...s.logs,
        [agentId]: [...(s.logs[agentId] ?? []), { kind: "queued_message", text: trimmed, turnId }],
      },
      busy: { ...s.busy, [agentId]: true },
    }));
    return guard(set, async () => {
      try {
        await api.sendUserMessage(agentId, turnId, trimmed);
      } catch (e) {
        set((s) => ({ busy: { ...s.busy, [agentId]: false } }));
        throw e;
      }
    });
  },

  async spawn({ repoPath, provider, model, effort, base, prompt, name: shown }) {
    return guard(set, async () => {
      // The user has been looking at (and possibly rerolled) this name; the
      // agent must launch under it, not a fresh draw.
      const name = shown || (await api.allocateDraftName([]));
      const record = await api.spawnAgent(repoPath, provider, name, effort, model, base);
      const turnId = newId();
      set((s) => ({
        workspace: s.workspace
          ? { ...s.workspace, agents: [record, ...s.workspace.agents] }
          : s.workspace,
        logs: { ...s.logs, [record.id]: [{ kind: "user_message", text: prompt, turnId }] },
        busy: { ...s.busy, [record.id]: true },
      }));
      get().closeSheet();
      get().push("agent", { agentId: record.id });
      try {
        await waitForSpawn(get, record.id);
        await api.sendUserMessage(record.id, turnId, prompt);
      } catch (e) {
        // The agent exists on the host but never got the prompt: drop the
        // optimistic turn and the busy flag, and let the refreshed workspace
        // show whatever state it is really in.
        set((s) => {
          const logs = { ...s.logs };
          const busy = { ...s.busy };
          delete logs[record.id];
          delete busy[record.id];
          return { logs, busy };
        });
        await get().refreshWorkspace();
        throw e;
      }
    });
  },

  async answerToolUse(agentId, toolUseId, updatedInput, behavior) {
    const requestId = get().pendingToolUse[agentId]?.[toolUseId];
    if (!requestId) return;
    set((s) => {
      const forAgent = { ...(s.pendingToolUse[agentId] ?? {}) };
      delete forAgent[toolUseId];
      return {
        pendingToolUse: { ...s.pendingToolUse, [agentId]: forAgent },
        busy: { ...s.busy, [agentId]: true },
      };
    });
    return guard(set, async () => {
      try {
        await api.answerToolUse(agentId, requestId, updatedInput, behavior);
      } catch (e) {
        set((s) => ({ busy: { ...s.busy, [agentId]: false } }));
        throw e;
      }
    });
  },

  async stop(agentId) {
    return guard(set, async () => {
      await api.stopAgent(agentId);
      set((s) => ({ busy: { ...s.busy, [agentId]: false } }));
    });
  },

  async resume(agentId) {
    return guard(set, async () => {
      await api.resumeAgent(agentId);
      set((s) => ({ busy: { ...s.busy, [agentId]: true } }));
    });
  },

  async archive(agentId) {
    return guard(set, async () => {
      await api.archiveAgent(agentId);
      get().closeSheet();
      get().pop();
      await get().refreshWorkspace();
    });
  },

  async setModel(agentId, model) {
    await guard(set, () => api.setAgentModel(agentId, model));
  },

  async setEffort(agentId, effort) {
    await guard(set, () => api.setAgentEffort(agentId, effort));
  },

  async publish(agentId, title, body) {
    const git = get().gitStates[agentId];
    if (git?.files.length) await api.commitAgent(agentId, title);
    await api.pushAgent(agentId);
    const pr = await api.createPr(agentId, title, body);
    set((s) => ({ prStates: { ...s.prStates, [agentId]: pr } }));
    await get().loadGit(agentId);
  },

  async pushToPr(agentId) {
    const git = get().gitStates[agentId];
    const title = get().prStates[agentId]?.title ?? "Update";
    if (git?.files.length) await api.commitAgent(agentId, title);
    await api.pushAgent(agentId);
    await get().loadGit(agentId);
  },

  async listDir(path) {
    return guard(set, () => api.listDir(path));
  },

  /** Both add-project ops answer with the whole new `Workspace`, which is
   *  applied here rather than waited for as a `workspace:changed`: it replaces
   *  the workspace wholesale, exactly as `refreshWorkspace` does, so an event
   *  landing either side of this leaves the same state. Only the Add Project
   *  sheet is closed on success — a slow clone must not dismiss a sheet the user
   *  opened in the meantime. */
  async addWorkspaceRepo(repoPath) {
    return guard(set, async () => {
      set({ workspace: await api.addWorkspaceRepo(repoPath) });
      get().closeSheetIf("addProject");
    });
  },

  async cloneRepo(spec, destParent) {
    return guard(set, async () => {
      const workspace = await api.cloneRepo(spec, destParent);
      set({ workspace, lastDestParent: destParent });
      await saveDestParent(get().hostKey, destParent);
      get().closeSheetIf("addProject");
    });
  },

  async ghStatus() {
    return guard(set, () => api.ghStatus());
  },

  async ghRepoList() {
    return guard(set, () => api.ghRepoList());
  },
}));

export type { RawEvent };
