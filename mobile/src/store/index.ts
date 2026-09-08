import type { AgentRecord, Workspace } from "@desktop/api/types/agent";
import type { CheckoutFile, DirListing } from "@desktop/api/types/checkout";
import type { DiffStats, GitState } from "@desktop/api/types/git";
import type { PrChecks, PrState } from "@desktop/api/types/pr";
import type { GhRepoSummary, GhStatus } from "@desktop/api/types/providers";
import { create } from "zustand";
import type { ChatItem, RawEvent } from "../adapters";
import { createApi } from "../api";
import { ignore } from "../lib/ignore";
import {
  type ConnectionState,
  createClient,
  DEFAULT_PORT,
  type HostInfo,
  type HostTarget,
  mockEnabled,
  type Via,
} from "../remote";
import { registerRemoteEvents } from "./events";
import { clearHost, loadDestParent, loadSettings, saveDestParent, saveSettings } from "./persist";
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
  reconnect(): Promise<void>;
  unpair(): Promise<void>;
  /** Add or change the relay for the paired host without re-pairing. */
  setRelay(url: string | null): Promise<void>;
  setTheme(theme: ThemeMode): void;
  setSystemTheme(t: "light" | "dark"): void;
  clearError(): void;

  push(screen: ScreenName, props?: Record<string, string>): void;
  pop(): void;
  openSheet(name: SheetName, props?: Record<string, string>): void;
  /** Close the open sheet. With `name`, only if that is the sheet showing: an
   *  async action finishing late must not dismiss whatever the user opened
   *  since. */
  closeSheet(name?: SheetName): void;

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
  setModel(agentId: string, model: string): Promise<void>;
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
}

let initialized = false;

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
    client.onState((state, error) =>
      set({
        connection: state,
        connectionError: error ?? null,
        hostInfo: client.host,
        via: client.via,
      }),
    );
    client.onSnapshot((snapshot) => {
      set({ hostInfo: snapshot.host });
      if (snapshot.workspace) set({ workspace: snapshot.workspace });
      else void get().refreshWorkspace();
    });
    if (typeof document !== "undefined") {
      document.addEventListener("visibilitychange", () => {
        if (!document.hidden && client.state === "connected") void get().refreshWorkspace();
      });
    }
    const saved = await loadSettings();
    set({ ready: true, theme: saved.theme ?? "dark", relay: saved.relay ?? null });
    if (mockEnabled()) {
      // The mock host has no pairing step worth clicking through every reload;
      // its fixed key is pinned by `connect` like any other.
      await get().connect({ host: "mock", port: DEFAULT_PORT }).catch(ignore);
      return;
    }
    // A saved host key is the whole credential: `hello` authenticates with the
    // device key the Rust layer holds.
    if (saved.host && saved.hostKey) {
      set({
        hostKey: saved.hostKey,
        lastDestParent: saved.destParents?.[saved.hostKey] ?? null,
      });
      await get()
        .connect({
          host: saved.host,
          port: saved.port ?? DEFAULT_PORT,
          name: saved.hostName,
          hostKey: saved.hostKey,
          // The relay is dialled only if the LAN address does not answer.
          relay: saved.relay,
        })
        .catch(ignore);
    }
  },

  async connect(target) {
    // The client owns the target from here: it strips the spent pairing token
    // and pins the host key the handshake authenticated.
    set({ connectionError: null });
    try {
      const snapshot = await client.connect(target);
      // Either the key the target carried, or the one just pinned.
      const hostKey = client.hostKey ?? target.hostKey ?? get().hostKey;
      const relay = client.target?.relay ?? null;
      set({
        hostInfo: snapshot.host,
        workspace: snapshot.workspace,
        hostKey,
        relay,
        via: client.via,
        // The remembered clone destination is per host, so it can only be
        // resolved once the handshake says which host this is.
        lastDestParent: await loadDestParent(hostKey),
        nav: [{ key: Date.now(), screen: "home", props: {}, phase: "idle" }],
      });
      // Mock mode must not leave a "mock" host behind for the next real run.
      if (!mockEnabled()) {
        await saveSettings({
          host: target.host,
          port: target.port,
          hostName: snapshot.host.name,
          relay: relay ?? undefined,
          ...(hostKey ? { hostKey } : {}),
        });
      }
      if (!snapshot.workspace) await get().refreshWorkspace();
    } catch (e) {
      set({ connectionError: message(e) });
      throw e;
    }
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
      nav: [{ key: Date.now(), screen: "home", props: {}, phase: "idle" }],
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

  openSheet(name, props = {}) {
    set({ sheet: { name, props, open: true } });
  },
  closeSheet(name) {
    set((s) =>
      s.sheet && (!name || s.sheet.name === name) ? { sheet: { ...s.sheet, open: false } } : s,
    );
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
    await get().rebuildLog(agentId);
    await get().loadGit(agentId);
  },

  async rebuildLog(agentId) {
    return guard(set, async () => {
      const [records, turns] = await Promise.all([
        api.readSessionRecords(agentId),
        api.readUserTurns(agentId),
      ]);
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
    // Optimistic bubble, reconciled away when the canonical records land.
    set((s) => ({
      logs: {
        ...s.logs,
        [agentId]: [...(s.logs[agentId] ?? []), { kind: "queued_message", text: trimmed }],
      },
      busy: { ...s.busy, [agentId]: true },
    }));
    return guard(set, async () => {
      try {
        await api.sendUserMessage(agentId, newId(), trimmed);
      } catch (e) {
        set((s) => ({ busy: { ...s.busy, [agentId]: false } }));
        throw e;
      }
    });
  },

  async spawn({ repoPath, provider, model, effort, base, prompt }) {
    return guard(set, async () => {
      const name = await api.allocateDraftName([]);
      const record = await api.spawnAgent(repoPath, provider, name, effort, model, base);
      set((s) => ({
        workspace: s.workspace
          ? { ...s.workspace, agents: [record, ...s.workspace.agents] }
          : s.workspace,
        logs: { ...s.logs, [record.id]: [{ kind: "user_message", text: prompt }] },
        busy: { ...s.busy, [record.id]: true },
      }));
      get().closeSheet();
      get().push("agent", { agentId: record.id });
      try {
        await waitForSpawn(get, record.id);
        await api.sendUserMessage(record.id, newId(), prompt);
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
      get().closeSheet("addProject");
    });
  },

  async cloneRepo(spec, destParent) {
    return guard(set, async () => {
      const workspace = await api.cloneRepo(spec, destParent);
      set({ workspace, lastDestParent: destParent });
      await saveDestParent(get().hostKey, destParent);
      get().closeSheet("addProject");
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
