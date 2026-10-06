// The store's `delegations` is a mirror of host facts (`supervisor::delegation`):
// the host decides, this window renders. What is left to pin is the mirroring —
// that a click reaches the host op, that every event shape lands where the
// panel reads it, that a resync can't be undone by an event it raced, and that
// a host without the op is told no rather than sent a trigger nobody watches.
// The decisions themselves are the host's tests now.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { delegateGit, getDelegations, getPrChecks } = vi.hoisted(() => ({
  delegateGit: vi.fn(),
  getDelegations: vi.fn(),
  getPrChecks: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { delegateGit, getDelegations, getPrChecks } }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));

import type { DelegationEvent } from "@/api";
import { dropAgentEntries } from "@/helpers/agentLookups";
import { type EnvironmentEntry, LOCAL_ENVIRONMENT_ID, setEnvironmentsSource } from "./environments";
import { checkoutKey, createGitSlice, splitCheckoutKey } from "./git";
import type { AppState } from "./types";

const remote = (ops: string[]): EnvironmentEntry => ({
  id: "host-1",
  name: "Cloud box",
  kind: "remote",
  connection: "connected",
  protocol: { version: 2, ops, events: [], features: [] },
});

const activeIs = (entry?: EnvironmentEntry) =>
  setEnvironmentsSource(() => ({
    activeEnvironmentId: entry?.id ?? LOCAL_ENVIRONMENT_ID,
    environments: entry ? { [entry.id]: entry } : {},
  }));

let stamp = 0;
const event = (over: Partial<DelegationEvent> = {}): DelegationEvent => ({
  agent_id: "a1",
  subdir: null,
  kind: "commit",
  phase: "started",
  started_at: ++stamp,
  ...over,
});

const makeStore = () => {
  const setLastError = vi.fn();
  const store = create<AppState>()(
    (...a) => ({ ...createGitSlice(...a), setLastError }) as unknown as AppState,
  );
  return { store, setLastError };
};

beforeEach(() => {
  delegateGit.mockReset();
  getDelegations.mockReset();
  getPrChecks.mockReset();
  activeIs();
});

describe("checkoutKey", () => {
  it("round-trips through splitCheckoutKey, primary and secondary alike", () => {
    expect(splitCheckoutKey(checkoutKey("a1"))).toEqual({ agentId: "a1" });
    expect(splitCheckoutKey(checkoutKey("a1", "web"))).toEqual({ agentId: "a1", subdir: "web" });
    // Splitting on the FIRST separator keeps a subdir containing "::" intact.
    expect(splitCheckoutKey(checkoutKey("a1", "od::d"))).toEqual({
      agentId: "a1",
      subdir: "od::d",
    });
  });
});

describe("delegateAction", () => {
  it("asks the host and mirrors what it recorded, per checkout", async () => {
    const { store } = makeStore();
    delegateGit.mockImplementation(async (agentId, action, _params, subdir) =>
      event({ agent_id: agentId, subdir: subdir ?? null, kind: action, phase: "queued" }),
    );

    await store.getState().delegateAction("a1", "commit");
    await store.getState().delegateAction("a1", "fix-checks", { failing: "unit" }, "web");

    expect(delegateGit).toHaveBeenNthCalledWith(1, "a1", "commit", undefined, undefined);
    expect(delegateGit).toHaveBeenNthCalledWith(2, "a1", "fix-checks", { failing: "unit" }, "web");
    expect(Object.keys(store.getState().delegations).sort()).toEqual(["a1", "a1::web"]);
    expect(store.getState().delegations["a1::web"]).toMatchObject({
      kind: "fix-checks",
      phase: "queued",
      subdir: "web",
    });
  });

  it("is told no by a host from before `delegate_git`, and sends nothing", async () => {
    activeIs(remote(["send_user_message"]));
    const { store, setLastError } = makeStore();

    await store.getState().delegateAction("a1", "commit");

    expect(delegateGit).not.toHaveBeenCalled();
    expect(setLastError).toHaveBeenCalledOnce();
    expect(store.getState().delegations).toEqual({});
  });

  it("surfaces a refusal from the host", async () => {
    const { store, setLastError } = makeStore();
    delegateGit.mockRejectedValue("unknown git action");

    await store.getState().delegateAction("a1", "rm-rf");

    expect(setLastError).toHaveBeenCalledWith("unknown git action");
    expect(store.getState().delegations).toEqual({});
  });

  it("does not resurrect a delegation whose end arrived before the reply", async () => {
    const { store } = makeStore();
    const recorded = event({ agent_id: "late" });
    delegateGit.mockImplementation(async () => {
      store.getState().applyDelegationChange({ ...recorded, phase: "done", notice: "Done" });
      return recorded;
    });

    await store.getState().delegateAction("late", "commit");

    expect(store.getState().delegations).toEqual({});
  });
});

describe("applyDelegationChange", () => {
  it("replaces the checkout's entry with each live phase", () => {
    const { store } = makeStore();
    store.getState().applyDelegationChange(event({ phase: "queued", started_at: 5 }));
    store.getState().applyDelegationChange(event({ phase: "started", started_at: 9 }));
    store.getState().applyDelegationChange(event({ phase: "running", started_at: 9 }));

    expect(store.getState().delegations).toEqual({
      a1: { kind: "commit", phase: "running", startedAt: 9 },
    });
  });

  it("drops it on its end and posts the host's notice for the panel", () => {
    const { store } = makeStore();
    store.getState().applyDelegationChange(event({ kind: "push", subdir: "web" }));
    store
      .getState()
      .applyDelegationChange(
        event({ kind: "push", subdir: "web", phase: "done", notice: "Pushed to origin" }),
      );

    expect(store.getState().delegations).toEqual({});
    expect(store.getState().delegationNotices).toEqual({ "a1::web": "Pushed to origin" });
    // A landed delegation moves the merge gate; the checks are re-read now.
    expect(getPrChecks).toHaveBeenCalledWith("a1", "web");
  });

  it("drops an orphan quietly", () => {
    const { store } = makeStore();
    store.getState().applyDelegationChange(event());
    store.getState().applyDelegationChange(event({ phase: "abandoned" }));

    expect(store.getState().delegations).toEqual({});
    expect(store.getState().delegationNotices).toEqual({});
    expect(getPrChecks).not.toHaveBeenCalled();
  });
});

describe("loadDelegations", () => {
  it("replaces the mirror with the host's table", async () => {
    const { store } = makeStore();
    store.getState().applyDelegationChange(event({ agent_id: "stale" }));
    getDelegations.mockResolvedValue([event({ agent_id: "a2", phase: "running" })]);

    await store.getState().loadDelegations();

    expect(Object.keys(store.getState().delegations)).toEqual(["a2"]);
  });

  it("keeps what the host said while the read was in flight", async () => {
    const { store } = makeStore();
    getDelegations.mockImplementation(async () => {
      store.getState().applyDelegationChange(event({ agent_id: "a2", phase: "done" }));
      store.getState().applyDelegationChange(event({ agent_id: "a3" }));
      return [event({ agent_id: "a2" })];
    });

    await store.getState().loadDelegations();

    expect(Object.keys(store.getState().delegations)).toEqual(["a3"]);
  });

  it("asks no host too old for the op", async () => {
    activeIs(remote(["send_user_message"]));
    const { store } = makeStore();

    await store.getState().loadDelegations();

    expect(getDelegations).not.toHaveBeenCalled();
  });
});

describe("dropAgentEntries", () => {
  it("takes every checkout's delegation and notice with the agent", () => {
    // An agent owns `id` AND `id::subdir` entries; a by-agent-key delete would
    // leave the secondaries behind.
    const live = { kind: "commit" as const, phase: "started" as const, startedAt: 1 };
    const patch = dropAgentEntries(
      {
        managedLogs: {},
        transcriptLoading: {},
        transcriptLoaded: {},
        sending: {},
        busyLabel: {},
        turnStartedAt: {},
        usage: {},
        gitStates: {},
        gitBlocked: {},
        prStates: {},
        prChecks: {},
        prComments: {},
        prSets: {},
        gitShortstats: {},
        composerSeeds: {},
        composerDrafts: {},
        delegations: { a1: live, "a1::web": { ...live, subdir: "web" }, a2: live },
        delegationNotices: { a1: "done", "a1::web": "done", a2: "done" },
        autopilot: {},
        autopilotLog: {},
        unseenResults: {},
        rightPanelTabs: {},
        offSidebarAgents: {},
        backgroundTasks: {},
        codeUndo: {},
        // biome-ignore lint/suspicious/noExplicitAny: partial state fixture
      } as any,
      "a1",
    );

    expect(Object.keys(patch.delegations ?? {})).toEqual(["a2"]);
    expect(Object.keys(patch.delegationNotices ?? {})).toEqual(["a2"]);
  });
});
