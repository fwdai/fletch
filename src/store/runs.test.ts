// The Run panel's state reaches the store by two paths that arrive in no
// guaranteed order: the `run:state` stream and the snapshot a panel fetches on
// mount. Both carry the backend's revision, and `applyRunState` keeps whichever
// is newer. These pin the two races that used to lose — a snapshot taken while
// a VM's forward was pending, resolving after the event that brought the port;
// a stale event from a stopped start landing after a newer run began — and the
// two compatibility edges: a host that restarted under the client, and a host
// from before revisions, whose port comes on `run:port` alone.

import { describe, expect, it, vi } from "vitest";
import { create } from "zustand";

vi.mock("@/api", () => ({ api: {} }));
vi.mock("@/pty/buffers", () => ({ clearOutputBuffer: vi.fn(), dropAgentPty: vi.fn() }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));
vi.mock("@/data/slashCommands", () => ({ discoverCommands: vi.fn() }));

import type { RunStateEvent, RunStateSnapshot, RunStatus } from "@/api/types/run";
import type { AppState } from "./types";
import { createWorkspaceSlice } from "./workspace";

const makeStore = () =>
  create<AppState>()((...a) => ({ ...createWorkspaceSlice(...a) }) as AppState);

/** A `run:state` from a current host: every field present, null when none. */
const event = (over: Partial<RunStateEvent>): RunStateEvent => ({
  agent_id: "a",
  phase: "running",
  last_error: null,
  port: null,
  guest_port: null,
  ports: [],
  revision: 1,
  ...over,
});

const snapshot = (over: Partial<RunStateSnapshot>): RunStateSnapshot => ({
  phase: "running",
  last_error: null,
  port: null,
  guest_port: null,
  ports: [],
  revision: 1,
  log: [],
  log_seq: 0,
  ...over,
});

const held = (over: Partial<RunStatus>): RunStatus => ({
  phase: "running",
  port: null,
  guest_port: null,
  ports: [],
  last_error: null,
  revision: 1,
  ...over,
});

describe("applyRunState", () => {
  it("keeps the port an event brought when an older snapshot resolves later", () => {
    const store = makeStore();
    // The forward settled: `run:state` says running on 3001, revision 7.
    store.getState().applyRunState("a", event({ port: 3001, guest_port: 3000, revision: 7 }));
    // The snapshot taken while the forward was pending, revision 6, arrives.
    store.getState().applyRunState("a", snapshot({ port: null, revision: 6 }));
    expect(store.getState().runs.a).toEqual(held({ port: 3001, guest_port: 3000, revision: 7 }));
  });

  it("drops a stopped start's late event once a newer run has begun", () => {
    const store = makeStore();
    store.getState().applyRunState("a", event({ phase: "stopped", revision: 9 }));
    store.getState().applyRunState("a", event({ phase: "running", revision: 10 }));
    // The old start settled its port just before the stop; its event is older.
    store.getState().applyRunState("a", event({ port: 3000, revision: 8 }));
    expect(store.getState().runs.a).toEqual(held({ phase: "running", revision: 10 }));
  });

  it("applies in order, replacing the whole status, and never regresses on an equal revision", () => {
    const store = makeStore();
    store.getState().applyRunState("a", event({ port: 3000, revision: 3 }));
    store.getState().applyRunState("a", event({ phase: "stopped", last_error: "x", revision: 4 }));
    expect(store.getState().runs.a).toEqual(
      held({ phase: "stopped", last_error: "x", revision: 4 }),
    );
    const before = store.getState().runs;
    store.getState().applyRunState("a", event({ port: 1, revision: 4 }));
    expect(store.getState().runs).toBe(before);
  });

  it("takes a restarted host's Idle snapshot over the run it remembers", () => {
    const store = makeStore();
    // Revisions are at least the clock: the old run's is yesterday's clock,
    // the restarted host's no-session Idle is today's.
    store.getState().applyRunState("a", event({ port: 3000, revision: 1_700_000_000_000 }));
    store
      .getState()
      .applyRunState("a", snapshot({ phase: "idle", port: null, revision: 1_700_000_100_000 }));
    expect(store.getState().runs.a).toEqual(held({ phase: "idle", revision: 1_700_000_100_000 }));
  });

  it("always applies a state from a host without revisions, in arrival order", () => {
    const store = makeStore();
    store.getState().applyRunState("a", event({ revision: 5 }));
    store.getState().applyRunState("a", event({ phase: "stopped", revision: undefined }));
    expect(store.getState().runs.a?.phase).toBe("stopped");
    store.getState().applyRunState("a", event({ phase: "running", revision: 2 }));
    expect(store.getState().runs.a?.phase).toBe("running");
  });
});

describe("applyRunState's forwarded ports", () => {
  const postgres = { guest_port: 5432, port: 5433 };
  const redis = { guest_port: 6379, port: 6379 };

  it("takes the ports a state brings, in any phase", () => {
    const store = makeStore();
    store.getState().applyRunState("a", event({ phase: "idle", ports: [postgres], revision: 2 }));
    expect(store.getState().runs.a).toEqual(
      held({ phase: "idle", ports: [postgres], revision: 2 }),
    );
  });

  it("replaces them with a newer revision's", () => {
    const store = makeStore();
    store.getState().applyRunState("a", event({ ports: [postgres, redis], revision: 2 }));
    store.getState().applyRunState("a", event({ ports: [redis], revision: 3 }));
    expect(store.getState().runs.a?.ports).toEqual([redis]);
  });

  it("keeps them when an older snapshot resolves later", () => {
    const store = makeStore();
    store.getState().applyRunState("a", event({ ports: [postgres], revision: 7 }));
    store.getState().applyRunState("a", snapshot({ ports: [], revision: 6 }));
    expect(store.getState().runs.a?.ports).toEqual([postgres]);
  });

  it("reads a state without the field as no ports", () => {
    const store = makeStore();
    store.getState().applyRunState("a", event({ ports: [postgres], revision: 2 }));
    store.getState().applyRunState("a", event({ ports: undefined, revision: 3 }));
    expect(store.getState().runs.a?.ports).toEqual([]);
  });
});

describe("applyRunPort", () => {
  /** A `run:state` from a host before ports and revisions rode it. */
  const legacy = (phase: RunStateEvent["phase"]): RunStateEvent =>
    ({ agent_id: "a", phase, last_error: null }) as RunStateEvent;

  it("gives a host from before revisions its port, in that host's event order", () => {
    const store = makeStore();
    // Such a host emits the port first, then the running state without one.
    store.getState().applyRunPort("a", { agent_id: "a", port: 3001 });
    expect(store.getState().runs.a).toEqual(held({ port: 3001, revision: null }));
    store.getState().applyRunState("a", legacy("running"));
    expect(store.getState().runs.a).toEqual(held({ port: 3001, revision: null }));
    store.getState().applyRunState("a", legacy("stopped"));
    expect(store.getState().runs.a?.phase).toBe("stopped");
    // A VM run on such a host: running first, the port once forwarded.
    store.getState().applyRunState("a", legacy("running"));
    store.getState().applyRunPort("a", { agent_id: "a", port: 3002, guest_port: 3000 });
    expect(store.getState().runs.a).toEqual(held({ port: 3002, guest_port: 3000, revision: null }));
  });

  it("is ignored for a host whose run:state carries the port", () => {
    const store = makeStore();
    store.getState().applyRunState("a", event({ phase: "stopped", revision: 9 }));
    store.getState().applyRunState("a", event({ phase: "running", revision: 10 }));
    // The stopped start's late `run:port`: nothing orders it, so it is not read.
    store.getState().applyRunPort("a", { agent_id: "a", port: 3000 });
    expect(store.getState().runs.a).toEqual(held({ phase: "running", revision: 10 }));
  });

  it("yields to the first ordered run:state when it arrived before any status", () => {
    const store = makeStore();
    // A current host's host-run start emits `run:port` just before `run:state`.
    store.getState().applyRunPort("a", { agent_id: "a", port: 3001 });
    store.getState().applyRunState("a", event({ port: 3001, revision: 4 }));
    expect(store.getState().runs.a).toEqual(held({ port: 3001, revision: 4 }));
  });
});
