// The store's autopilot is a mirror of host facts (docs/remote-protocol.md,
// "Autopilot"): the host runs the loop, this window renders it and flips its
// switches. What is left to pin is the mirroring — that every event lands where
// the panel reads it, that a resync can't be undone by an event it raced, and
// that a switch reaches the host op and settles on its answer. The decisions
// themselves are the host's tests now.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { getAutopilotState, getAutopilotLog, setAutopilot } = vi.hoisted(() => ({
  getAutopilotState: vi.fn(),
  getAutopilotLog: vi.fn(),
  setAutopilot: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { getAutopilotState, getAutopilotLog, setAutopilot } }));

import type { AutopilotCheckout, AutopilotLogEntry, AutopilotSnapshot } from "@/api";
import { dropAgentEntries } from "@/helpers/agentLookups";
import { AUTOPILOT_LOG_LIMIT, autopilotProjectOn, createAutopilotSlice } from "./autopilot";
import { type EnvironmentEntry, LOCAL_ENVIRONMENT_ID, setEnvironmentsSource } from "./environments";
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

const row = (over: Partial<AutopilotCheckout> = {}): AutopilotCheckout => ({
  agent_id: "a1",
  subdir: null,
  project_id: "p1",
  enrolled: true,
  paused: false,
  project_enabled: true,
  cycle: null,
  ...over,
});

let stamp = 0;
const entry = (over: Partial<AutopilotLogEntry> = {}): AutopilotLogEntry => {
  stamp++;
  return {
    id: `e${stamp}`,
    agent_id: "a1",
    subdir: null,
    at: stamp,
    outcome: "dispatch",
    rung: "fix-checks",
    attempt: 1,
    ...over,
  };
};

const snapshot = (over: Partial<AutopilotSnapshot> = {}): AutopilotSnapshot => ({
  checkouts: [],
  disabled_projects: [],
  paused_agents: [],
  ...over,
});

const makeStore = () => {
  const setLastError = vi.fn();
  const store = create<AppState>()(
    (...a) => ({ ...createAutopilotSlice(...a), setLastError }) as unknown as AppState,
  );
  return { store, setLastError };
};

/** An async answer the test releases by hand, to order completions. */
const deferred = <T>() => {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
};

beforeEach(() => {
  getAutopilotState.mockReset();
  getAutopilotLog.mockReset().mockResolvedValue([]);
  setAutopilot.mockReset();
  activeIs();
});

describe("autopilotProjectOn", () => {
  it("is on by default, off when opted out, and off while unknown", () => {
    expect(autopilotProjectOn([], "p1")).toBe(true);
    expect(autopilotProjectOn(["p1"], "p1")).toBe(false);
    expect(autopilotProjectOn(null, "p1")).toBe(false);
  });
});

describe("applyAutopilotState", () => {
  it("replaces the row for its checkout, primary and secondary alike", () => {
    const { store } = makeStore();
    store.getState().applyAutopilotState(row());
    store.getState().applyAutopilotState(row({ subdir: "web" }));
    const working = {
      rung: "fix-checks" as const,
      attempt: 2,
      phase: "working" as const,
      since: 9,
    };
    store.getState().applyAutopilotState(row({ cycle: working }));

    expect(Object.keys(store.getState().autopilot).sort()).toEqual(["a1", "a1::web"]);
    expect(store.getState().autopilot.a1.cycle).toEqual(working);
  });

  it("keeps the opt-out lists in step with the row", () => {
    const { store } = makeStore();
    store.setState({ autopilotDisabledProjects: [] });

    store.getState().applyAutopilotState(row({ paused: true, project_enabled: false }));
    expect(store.getState().autopilotPausedAgents).toEqual(["a1"]);
    expect(store.getState().autopilotDisabledProjects).toEqual(["p1"]);

    store.getState().applyAutopilotState(row());
    expect(store.getState().autopilotPausedAgents).toEqual([]);
    expect(store.getState().autopilotDisabledProjects).toEqual([]);
  });

  it("leaves unknown opt-outs unknown — one row says nothing of other projects", () => {
    const { store } = makeStore();
    store.getState().applyAutopilotState(row({ project_enabled: false }));
    expect(store.getState().autopilotDisabledProjects).toBeNull();
  });
});

describe("applyAutopilotEvent", () => {
  it("keeps each checkout's history newest first, and apart", () => {
    const { store } = makeStore();
    const first = entry();
    const second = entry({ outcome: "settle" });
    const web = entry({ subdir: "web" });
    for (const e of [first, second, web]) store.getState().applyAutopilotEvent(e);

    expect(store.getState().autopilotLog.a1).toEqual([second, first]);
    expect(store.getState().autopilotLog["a1::web"]).toEqual([web]);
  });

  it("ignores a row it already holds", () => {
    const { store } = makeStore();
    const e = entry();
    store.getState().applyAutopilotEvent(e);
    store.getState().applyAutopilotEvent(e);
    expect(store.getState().autopilotLog.a1).toHaveLength(1);
  });

  it(`keeps at most ${AUTOPILOT_LOG_LIMIT} rows per checkout, dropping the oldest`, () => {
    const { store } = makeStore();
    const all = Array.from({ length: AUTOPILOT_LOG_LIMIT + 3 }, () => entry());
    for (const e of all) store.getState().applyAutopilotEvent(e);

    const log = store.getState().autopilotLog.a1;
    expect(log).toHaveLength(AUTOPILOT_LOG_LIMIT);
    expect(log[0]).toBe(all.at(-1));
    expect(log).not.toContain(all[0]);
  });
});

describe("loadAutopilot", () => {
  it("replaces the mirror with the host's state and history", async () => {
    const { store } = makeStore();
    store.getState().applyAutopilotState(row({ agent_id: "stale" }));
    const newer = entry({ outcome: "settle" });
    const older = entry();
    getAutopilotState.mockResolvedValue(
      snapshot({
        checkouts: [row({ agent_id: "a2", subdir: "web" })],
        disabled_projects: ["p9"],
        paused_agents: ["a3"],
      }),
    );
    getAutopilotLog.mockResolvedValue([newer, older]);

    await store.getState().loadAutopilot();

    const s = store.getState();
    expect(Object.keys(s.autopilot)).toEqual(["a2::web"]);
    expect(s.autopilotDisabledProjects).toEqual(["p9"]);
    expect(s.autopilotPausedAgents).toEqual(["a3"]);
    expect(s.autopilotLog).toEqual({ a1: [newer, older] });
  });

  it("keeps what the host said while the read was in flight", async () => {
    const { store } = makeStore();
    const before = entry();
    const during = entry({ outcome: "settle" });
    getAutopilotState.mockImplementation(async () => {
      store.getState().applyAutopilotState(row({ paused: true }));
      store.getState().applyAutopilotEvent(during);
      return snapshot({ checkouts: [row()] });
    });
    getAutopilotLog.mockResolvedValue([before]);

    await store.getState().loadAutopilot();

    expect(store.getState().autopilot.a1.paused).toBe(true);
    expect(store.getState().autopilotPausedAgents).toEqual(["a1"]);
    expect(store.getState().autopilotLog.a1).toEqual([during, before]);
  });

  it("lets the newest of two overlapping loads own the mirror", async () => {
    const { store } = makeStore();
    const answers = [deferred<AutopilotSnapshot>(), deferred<AutopilotSnapshot>()];
    getAutopilotState
      .mockReturnValueOnce(answers[0].promise)
      .mockReturnValueOnce(answers[1].promise);

    const older = store.getState().loadAutopilot();
    const newer = store.getState().loadAutopilot();
    answers[1].resolve(snapshot({ paused_agents: ["from-the-newer-read"] }));
    await newer;
    answers[0].resolve(snapshot({ paused_agents: ["from-the-older-read"] }));
    await older;

    expect(store.getState().autopilotPausedAgents).toEqual(["from-the-newer-read"]);
  });

  it("stays unknown when the read fails", async () => {
    const { store } = makeStore();
    getAutopilotState.mockRejectedValue(new Error("not connected"));

    await store.getState().loadAutopilot();

    expect(store.getState().autopilotDisabledProjects).toBeNull();
  });

  it("asks no host too old for the op", async () => {
    activeIs(remote(["send_user_message"]));
    const { store } = makeStore();

    await store.getState().loadAutopilot();

    expect(getAutopilotState).not.toHaveBeenCalled();
  });
});

describe("the switches", () => {
  it("ask the host, by project or by agent, and settle on its answer", async () => {
    const { store } = makeStore();
    store.setState({ autopilotDisabledProjects: [] });
    setAutopilot.mockResolvedValueOnce(snapshot({ disabled_projects: ["p1"] }));
    setAutopilot.mockResolvedValueOnce(
      snapshot({ disabled_projects: ["p1"], paused_agents: ["a1"] }),
    );

    await store.getState().setProjectAutopilot("p1", false);
    await store.getState().setAgentAutopilot("a1", false);

    expect(setAutopilot).toHaveBeenNthCalledWith(1, { projectId: "p1" }, false);
    expect(setAutopilot).toHaveBeenNthCalledWith(2, { agentId: "a1" }, false);
    expect(store.getState().autopilotDisabledProjects).toEqual(["p1"]);
    expect(store.getState().autopilotPausedAgents).toEqual(["a1"]);
  });

  it("answer at once, before the host does", () => {
    const { store } = makeStore();
    setAutopilot.mockReturnValue(new Promise(() => {}));

    void store.getState().setAgentAutopilot("a1", false);

    expect(store.getState().autopilotPausedAgents).toEqual(["a1"]);
  });

  it("put the switch back, and say why, when the host refuses", async () => {
    const { store, setLastError } = makeStore();
    store.setState({ autopilotDisabledProjects: [] });
    setAutopilot.mockRejectedValue("forbidden");

    await store.getState().setProjectAutopilot("p1", false);

    expect(store.getState().autopilotDisabledProjects).toEqual([]);
    expect(setLastError).toHaveBeenCalledWith("forbidden");
  });

  it("do not let a slow earlier answer undo a later click", async () => {
    const { store } = makeStore();
    const first = deferred<AutopilotSnapshot>();
    setAutopilot
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce(snapshot({ paused_agents: [] }));

    const pause = store.getState().setAgentAutopilot("a1", false);
    await store.getState().setAgentAutopilot("a1", true);
    first.resolve(snapshot({ paused_agents: ["a1"] }));
    await pause;

    expect(store.getState().autopilotPausedAgents).toEqual([]);
  });

  it("are refused on a host too old to run autopilot, and send nothing", async () => {
    activeIs(remote(["send_user_message"]));
    const { store, setLastError } = makeStore();

    await store.getState().setAgentAutopilot("a1", false);

    expect(setAutopilot).not.toHaveBeenCalled();
    expect(setLastError).toHaveBeenCalledOnce();
    expect(store.getState().autopilotPausedAgents).toEqual([]);
  });
});

describe("dropAgentEntries", () => {
  it("takes every checkout's row and history with the agent", () => {
    const { store } = makeStore();
    for (const r of [row(), row({ subdir: "web" }), row({ agent_id: "a2" })]) {
      store.getState().applyAutopilotState(r);
      store.getState().applyAutopilotEvent(entry({ agent_id: r.agent_id, subdir: r.subdir }));
    }
    const s = store.getState();

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
        gitShortstats: {},
        composerSeeds: {},
        composerDrafts: {},
        delegations: {},
        delegationNotices: {},
        autopilot: s.autopilot,
        autopilotLog: s.autopilotLog,
        unseenResults: {},
        rightPanelTabs: {},
        offSidebarAgents: {},
        backgroundTasks: {},
        codeUndo: {},
        // biome-ignore lint/suspicious/noExplicitAny: partial state fixture
      } as any,
      "a1",
    );

    expect(Object.keys(patch.autopilot ?? {})).toEqual(["a2"]);
    expect(Object.keys(patch.autopilotLog ?? {})).toEqual(["a2"]);
  });
});
