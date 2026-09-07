import type { AgentRecord, Workspace } from "@desktop/api/types/agent";
import type { CheckoutFile } from "@desktop/api/types/checkout";
import type { DiffStats, GitState } from "@desktop/api/types/git";
import type { PrChecks, PrState } from "@desktop/api/types/pr";
import { create } from "zustand";
import type { ChatItem, RawEvent } from "../adapters";
import { createApi } from "../api";
import {
  type ConnectionState,
  createClient,
  DEFAULT_PORT,
  type HostInfo,
  type HostTarget,
  mockEnabled,
} from "../remote";
import { registerRemoteEvents } from "./events";
import { clearCredentials, loadSettings, saveSettings } from "./persist";
import { applyUserTurns, reduceRecords } from "./transcript";

const client = createClient();
export const api = createApi(client);

export type ThemeMode = "system" | "light" | "dark";
export type ScreenName = "home" | "project" | "agent" | "file" | "diff";
export type SheetName = "host" | "newAgent" | "agentMore" | "modelPicker" | "pr";

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
  target: HostTarget | null;
  deviceToken: string | null;

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
  setTheme(theme: ThemeMode): void;
  setSystemTheme(t: "light" | "dark"): void;
  clearError(): void;

  push(screen: ScreenName, props?: Record<string, string>): void;
  pop(): void;
  openSheet(name: SheetName, props?: Record<string, string>): void;
  closeSheet(): void;

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
  target: null,
  deviceToken: null,

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
      set({ connection: state, connectionError: error ?? null, hostInfo: client.host }),
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
    set({ ready: true, theme: saved.theme ?? "dark" });
    if (mockEnabled()) {
      // The mock host has no pairing step worth clicking through every reload.
      set({ deviceToken: "mock" });
      await get()
        .connect({ host: "mock", port: DEFAULT_PORT, deviceToken: "mock" })
        .catch(() => {});
      return;
    }
    if (saved.host && saved.deviceToken) {
      set({ deviceToken: saved.deviceToken });
      await get()
        .connect({
          host: saved.host,
          port: saved.port ?? 47285,
          name: saved.hostName,
          deviceToken: saved.deviceToken,
        })
        .catch(() => {});
    }
  },

  async connect(target) {
    set({ target, connectionError: null });
    try {
      const snapshot = await client.connect(target);
      // After a `pair` handshake the client holds the freshly minted token.
      const deviceToken = client.deviceToken ?? target.deviceToken ?? get().deviceToken;
      set({
        hostInfo: snapshot.host,
        workspace: snapshot.workspace,
        deviceToken,
        nav: [{ key: Date.now(), screen: "home", props: {}, phase: "idle" }],
      });
      // Mock mode must not leave a "mock" host behind for the next real run.
      if (!mockEnabled()) {
        await saveSettings({
          host: target.host,
          port: target.port,
          hostName: snapshot.host.name,
          ...(deviceToken ? { deviceToken } : {}),
        });
      }
      if (!snapshot.workspace) await get().refreshWorkspace();
    } catch (e) {
      set({ connectionError: e instanceof Error ? e.message : String(e) });
      throw e;
    }
  },

  async reconnect() {
    const target = get().target;
    if (target)
      await get()
        .connect(target)
        .catch(() => {});
  },

  async unpair() {
    client.disconnect();
    await clearCredentials();
    set({
      deviceToken: null,
      target: null,
      workspace: null,
      hostInfo: null,
      logs: {},
      sheet: null,
      nav: [{ key: Date.now(), screen: "home", props: {}, phase: "idle" }],
    });
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
  closeSheet() {
    set((s) => (s.sheet ? { sheet: { ...s.sheet, open: false } } : s));
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
    void get().loadAgent(agentId);
  },

  async loadAgent(agentId) {
    await get().rebuildLog(agentId);
    await get().loadGit(agentId);
  },

  async rebuildLog(agentId) {
    try {
      const [records, turns] = await Promise.all([
        api.readSessionRecords(agentId),
        api.readUserTurns(agentId),
      ]);
      const provider = agentOf(get().workspace, agentId)?.provider;
      const items = applyUserTurns(reduceRecords(provider, records), turns);
      set((s) => ({ logs: { ...s.logs, [agentId]: items } }));
    } catch (e) {
      set({ lastError: e instanceof Error ? e.message : String(e) });
    }
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
    try {
      const tree = await api.listCheckoutTree(agentId);
      set((s) => ({ trees: { ...s.trees, [agentId]: tree } }));
    } catch (e) {
      set({ lastError: e instanceof Error ? e.message : String(e) });
    }
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
    try {
      await api.sendUserMessage(agentId, newId(), trimmed);
    } catch (e) {
      set((s) => ({
        lastError: e instanceof Error ? e.message : String(e),
        busy: { ...s.busy, [agentId]: false },
      }));
    }
  },

  async spawn({ repoPath, provider, model, effort, base, prompt }) {
    try {
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
      await waitForSpawn(get, record.id);
      await api.sendUserMessage(record.id, newId(), prompt);
    } catch (e) {
      set({ lastError: e instanceof Error ? e.message : String(e) });
    }
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
    try {
      await api.answerToolUse(agentId, requestId, updatedInput, behavior);
    } catch (e) {
      set((s) => ({
        lastError: e instanceof Error ? e.message : String(e),
        busy: { ...s.busy, [agentId]: false },
      }));
    }
  },

  async stop(agentId) {
    try {
      await api.stopAgent(agentId);
      set((s) => ({ busy: { ...s.busy, [agentId]: false } }));
    } catch (e) {
      set({ lastError: e instanceof Error ? e.message : String(e) });
    }
  },

  async resume(agentId) {
    try {
      await api.resumeAgent(agentId);
      set((s) => ({ busy: { ...s.busy, [agentId]: true } }));
    } catch (e) {
      set({ lastError: e instanceof Error ? e.message : String(e) });
    }
  },

  async archive(agentId) {
    try {
      await api.archiveAgent(agentId);
      get().closeSheet();
      get().pop();
      await get().refreshWorkspace();
    } catch (e) {
      set({ lastError: e instanceof Error ? e.message : String(e) });
    }
  },

  async setModel(agentId, model) {
    try {
      await api.setAgentModel(agentId, model);
    } catch (e) {
      set({ lastError: e instanceof Error ? e.message : String(e) });
    }
  },

  async setEffort(agentId, effort) {
    try {
      await api.setAgentEffort(agentId, effort);
    } catch (e) {
      set({ lastError: e instanceof Error ? e.message : String(e) });
    }
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
}));

export type { RawEvent };
