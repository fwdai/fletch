// The Run panel's state reaches the store by two paths that arrive in no
// guaranteed order: the `run:state` stream and the snapshot a panel fetches on
// mount. Both carry the backend's revision, and `applyRunState` keeps whichever
// is newer. These pin the two races that used to lose: a snapshot taken while a
// VM's forward was pending, resolving after the event that brought the port;
// and a stale event from a stopped start landing after a newer run began.

import { describe, expect, it, vi } from "vitest";
import { create } from "zustand";

vi.mock("@/api", () => ({ api: {} }));
vi.mock("@/pty/buffers", () => ({ clearOutputBuffer: vi.fn(), dropAgentPty: vi.fn() }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));
vi.mock("@/data/slashCommands", () => ({ discoverCommands: vi.fn() }));

import { type RunStateEvent, type RunStatus, toRunStatus } from "@/api/types/run";
import type { AppState } from "./types";
import { createWorkspaceSlice } from "./workspace";

const makeStore = () =>
  create<AppState>()((...a) => ({ ...createWorkspaceSlice(...a) }) as AppState);

const status = (over: Partial<RunStatus>): RunStatus => ({
  phase: "running",
  port: null,
  guest_port: null,
  last_error: null,
  revision: 1,
  ...over,
});

describe("applyRunState", () => {
  it("keeps the port an event brought when an older snapshot resolves later", () => {
    const store = makeStore();
    // The forward settled: `run:state` says running on 3001, revision 7.
    store.getState().applyRunState("a", status({ port: 3001, guest_port: 3000, revision: 7 }));
    // The snapshot taken while the forward was pending, revision 6, arrives.
    store.getState().applyRunState("a", status({ port: null, revision: 6 }));
    expect(store.getState().runs.a).toEqual(status({ port: 3001, guest_port: 3000, revision: 7 }));
  });

  it("drops a stopped start's late event once a newer run has begun", () => {
    const store = makeStore();
    store.getState().applyRunState("a", status({ phase: "stopped", revision: 9 }));
    store.getState().applyRunState("a", status({ phase: "running", revision: 10 }));
    // The old start settled its port just before the stop; its event is older.
    store.getState().applyRunState("a", status({ port: 3000, revision: 8 }));
    expect(store.getState().runs.a).toEqual(status({ phase: "running", revision: 10 }));
  });

  it("applies in order, replacing the whole status, and never regresses on an equal revision", () => {
    const store = makeStore();
    store.getState().applyRunState("a", status({ port: 3000, revision: 3 }));
    store.getState().applyRunState("a", status({ phase: "stopped", last_error: "x", revision: 4 }));
    expect(store.getState().runs.a).toEqual(
      status({ phase: "stopped", last_error: "x", revision: 4 }),
    );
    const before = store.getState().runs;
    store.getState().applyRunState("a", status({ port: 1, revision: 4 }));
    expect(store.getState().runs).toBe(before);
  });

  it("always applies a state from a host without revisions, in arrival order", () => {
    const store = makeStore();
    store.getState().applyRunState("a", status({ revision: 5 }));
    store.getState().applyRunState("a", status({ phase: "stopped", revision: null }));
    expect(store.getState().runs.a?.phase).toBe("stopped");
    store.getState().applyRunState("a", status({ phase: "running", revision: 2 }));
    expect(store.getState().runs.a?.phase).toBe("running");
  });

  it("reads an older host's event with null ports and revision", () => {
    const legacy = { agent_id: "a", phase: "running", last_error: null } as RunStateEvent;
    expect(toRunStatus(legacy)).toEqual(status({ revision: null }));
  });
});
