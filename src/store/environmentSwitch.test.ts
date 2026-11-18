// What a switch and a handshake have to get right. Every host's state is live at
// once, so a switch only changes which one is on screen — at once, touching
// nothing else and asking no engine anything. A handshake is where a host's
// state is re-read: through that host's own actions, whether or not it is on
// screen, and never through anyone else's.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { getWorkspace } = vi.hoisted(() => ({ getWorkspace: vi.fn() }));
vi.mock("@/api", () => ({ api: { getWorkspace } }));

import { testEngine } from "@/test/engine";
import { createEnvironmentSwitchSlice } from "./environmentSwitch";
import {
  createEnvironmentsSlice,
  type EnvironmentEntry,
  type EnvironmentId,
  LOCAL_ENVIRONMENT_ID,
  setEnvironmentsSource,
} from "./environments";
import type { AppState } from "./types";

const HOST = "host-key-1";

const remoteEntry = (over: Partial<EnvironmentEntry> = {}): EnvironmentEntry => ({
  id: HOST,
  name: "Cloud box",
  kind: "remote",
  connection: "connected",
  ...over,
});

/** The two slices under test, acting for `hostId`, over stand-ins for every
 *  read a handshake makes. */
const newStore = (hostId: EnvironmentId = LOCAL_ENVIRONMENT_ID) => {
  const store = create<AppState>()(
    (set, get) =>
      ({
        ...createEnvironmentsSlice(set, get, testEngine(hostId)),
        ...createEnvironmentSwitchSlice(set, get, testEngine(hostId)),
        workspace: null,
        selectedAgentId: null,
        offSidebarAgents: {},
        managedLogs: {},
        sending: {},
        turnStartedAt: {},
        loadHistoryTranscript: vi.fn().mockResolvedValue(undefined),
        loadPendingPublishApprovals: vi.fn().mockResolvedValue(undefined),
        loadPendingEgressPrompts: vi.fn().mockResolvedValue(undefined),
        loadDelegations: vi.fn().mockResolvedValue(undefined),
        loadAutopilot: vi.fn().mockResolvedValue(undefined),
        loadReviews: vi.fn().mockResolvedValue(undefined),
        refreshGithub: vi.fn().mockResolvedValue(undefined),
        loadAllPrStatus: vi.fn().mockResolvedValue(undefined),
        loadBoxSetup: vi.fn().mockResolvedValue(undefined),
      }) as unknown as AppState,
  );
  setEnvironmentsSource(store.getState);
  store.setState({ environments: { ...store.getState().environments, [HOST]: remoteEntry() } });
  return store;
};

/** The handshake's reads, as the stand-ins recorded them. */
const reads = (store: ReturnType<typeof newStore>) => {
  const s = store.getState();
  return [
    s.loadPendingPublishApprovals,
    s.loadPendingEgressPrompts,
    s.loadDelegations,
    s.loadAutopilot,
    s.loadReviews,
    s.refreshGithub,
    s.loadAllPrStatus,
  ].map((read) => vi.mocked(read).mock.calls.length);
};

// Minimal workspace shapes; nothing here reads past the identity and the (empty)
// agent list the snapshot reconcile walks.
const ws = (label: string) =>
  ({ label, agents: [] }) as unknown as NonNullable<AppState["workspace"]>;

/** `environmentReconnected` is fire-and-forget; let its awaits land. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  getWorkspace.mockReset();
  getWorkspace.mockResolvedValue(null);
});

describe("switchEnvironment", () => {
  it("puts another environment on screen at once, touching nothing else", async () => {
    const store = newStore();
    store.setState({ workspace: ws("local"), selectedAgentId: "fuji" });
    const before = store.getState();

    const result = store.getState().switchEnvironment(HOST);

    // Synchronous: done by the time the call returns, with nothing to await.
    expect(result).toBeUndefined();
    expect(store.getState().activeEnvironmentId).toBe(HOST);
    // Nothing parked, restored or blanked: what was there is still there.
    expect(store.getState().workspace).toBe(before.workspace);
    expect(store.getState().selectedAgentId).toBe("fuji");
    expect(store.getState().environments).toBe(before.environments);

    await settle();
    // And no engine is asked anything.
    expect(getWorkspace).not.toHaveBeenCalled();
    expect(reads(store)).toEqual([0, 0, 0, 0, 0, 0, 0]);
    expect(store.getState().loadBoxSetup).not.toHaveBeenCalled();
  });

  it("ignores a switch to the active environment or to one that is gone", () => {
    const store = newStore();
    const before = store.getState();

    store.getState().switchEnvironment(LOCAL_ENVIRONMENT_ID);
    store.getState().switchEnvironment("a host forgotten a moment ago");

    expect(store.getState()).toBe(before);
  });
});

describe("environmentReconnected", () => {
  it("re-reads the host's own state, whether or not it is on screen", async () => {
    // This Mac is on screen; the handshake is the paired host's, and its
    // store is its own.
    const store = newStore(HOST);
    store.setState({ selectedAgentId: "fuji" });
    getWorkspace.mockResolvedValue(ws("from the host"));

    store.getState().environmentReconnected(HOST);
    await settle();

    expect(store.getState().activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);
    expect(getWorkspace).toHaveBeenCalledOnce();
    expect(store.getState().workspace).toEqual(ws("from the host"));
    // What it is blocked on, what it runs, whose GitHub login it has and its
    // PR badges: everything it owns the answer to.
    expect(reads(store)).toEqual([1, 1, 1, 1, 1, 1, 1]);
    // The open agent's log, whose live events a dropped socket missed.
    expect(store.getState().loadHistoryTranscript).toHaveBeenCalledWith("fuji");
  });

  it("asks for the PR badges only once the GitHub login is known", async () => {
    const store = newStore(HOST);

    store.getState().environmentReconnected(HOST);
    await settle();

    // The PR seed is GitHub-gated, and re-checks closed PRs at once rather
    // than waiting for the host's slow safety-net pass.
    const probe = vi.mocked(store.getState().refreshGithub);
    const seed = vi.mocked(store.getState().loadAllPrStatus);
    expect(seed).toHaveBeenCalledWith(true);
    expect(probe.mock.invocationCallOrder[0]).toBeLessThan(seed.mock.invocationCallOrder[0]);
  });

  it("carries on past a host that cannot answer", async () => {
    const store = newStore(HOST);
    store.setState({ workspace: ws("last seen") });
    // What a client with no socket does: rejects at once.
    getWorkspace.mockRejectedValue(new Error("not connected"));

    store.getState().environmentReconnected(HOST);
    await settle();

    expect(store.getState().workspace).toEqual(ws("last seen"));
    expect(reads(store)).toEqual([1, 1, 1, 1, 1, 1, 1]);
  });

  it("leaves another host's handshake to that host", async () => {
    const store = newStore(LOCAL_ENVIRONMENT_ID);

    store.getState().environmentReconnected(HOST);
    await settle();

    expect(getWorkspace).not.toHaveBeenCalled();
    expect(reads(store)).toEqual([0, 0, 0, 0, 0, 0, 0]);
  });

  it("keeps a mid-turn log rather than replacing it with the prompt alone", async () => {
    const store = newStore(HOST);
    store.setState({
      selectedAgentId: "fuji",
      sending: { fuji: true },
      managedLogs: { fuji: [] },
    });

    store.getState().environmentReconnected(HOST);
    await settle();

    expect(store.getState().loadHistoryTranscript).not.toHaveBeenCalled();
  });
});
