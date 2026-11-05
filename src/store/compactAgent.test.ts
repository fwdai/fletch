// Compaction against a real store. `/compact` must open a turn of its own: a
// send waiting in line has not raised `sending` yet, so the agent still reads
// idle, and these pin that compaction is refused behind one and that a
// `/compact` whose agent turned busy before its place in line came is dropped
// rather than delivered as a mid-turn follow-up. Then the stages: how long the
// badge says "Compacting…", across turn boundaries and silences.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { sendUserMessage, grantAgentFolder } = vi.hoisted(() => ({
  sendUserMessage: vi.fn(),
  grantAgentFolder: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { sendUserMessage, grantAgentFolder } }));
vi.mock("@/pty/buffers", () => ({ clearOutputBuffer: vi.fn(), dropAgentPty: vi.fn() }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));
vi.mock("@/data/slashCommands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/data/slashCommands")>()),
  discoverCommands: vi.fn(),
}));

import type { RawEvent } from "@/adapters/types";
import type { AgentRecord, FolderGrant } from "@/api";
import { COMPACTION_QUIET_MS, COMPACTION_SETTLE_MS, createCompactionSlice } from "./compaction";
import type { AppState } from "./types";
import { createWorkspaceSlice } from "./workspace";

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

const agent = {
  id: "a1",
  status: "idle",
  provider: "claude",
  view: "custom",
  repos: [],
  folder_grants: [],
} as unknown as AgentRecord;

const makeStore = () => {
  const store = create<AppState>()(
    (...a) => ({ ...createWorkspaceSlice(...a), ...createCompactionSlice(...a) }) as AppState,
  );
  store.setState({
    workspace: { agents: [agent], projects: [] },
    offSidebarAgents: {},
    managedLogs: {},
    sending: {},
    busyLabel: {},
    transcriptLoading: {},
    switchInFlight: {},
    features: { nativeView: false },
  } as unknown as Partial<AppState>);
  return store;
};

const sentTexts = () => sendUserMessage.mock.calls.map((c) => c[2]);
const lib: FolderGrant = { path: "/Users/u/Projects/lib", writable: false };

beforeEach(() => {
  vi.clearAllMocks();
  sendUserMessage.mockResolvedValue(false);
});

describe("compactAgent", () => {
  it("sends /compact to an idle agent", async () => {
    const store = makeStore();
    await store.getState().compactAgent("a1");
    expect(sentTexts()).toEqual(["/compact"]);
  });

  it("refuses behind an earlier send still waiting in line", async () => {
    const grant = deferred<unknown>();
    grantAgentFolder.mockReturnValue(grant.promise);
    const store = makeStore();

    // Stalled on its folder grant, before it raises `sending`: the agent
    // still reads idle.
    const first = store.getState().sendUserMessage("a1", "read the lib", [], [lib]);
    await store.getState().compactAgent("a1");

    grant.resolve({
      granted: lib.path,
      folder_grants: [{ path: lib.path, writable: false }],
      applies_at_turn_end: false,
    });
    await first;
    expect(sentTexts()).toEqual(["read the lib"]);
  });
});

describe("sendUserMessage idleOnly", () => {
  it("drops the message, bubble and all, if the agent is busy when its turn comes", async () => {
    const grant = deferred<unknown>();
    grantAgentFolder.mockReturnValue(grant.promise);
    const store = makeStore();

    const first = store.getState().sendUserMessage("a1", "read the lib", [], [lib]);
    const compact = store.getState().sendUserMessage("a1", "/compact", [], [], { idleOnly: true });

    grant.resolve({
      granted: lib.path,
      folder_grants: [{ path: lib.path, writable: false }],
      applies_at_turn_end: false,
    });
    await first;
    await expect(compact).resolves.toBe(false);
    expect(sentTexts()).toEqual(["read the lib"]);
    const log = store.getState().managedLogs.a1 ?? [];
    expect(log.some((it) => "text" in it && it.text === "/compact")).toBe(false);
    expect(store.getState().lastError).toMatch(/wasn't sent/);
  });
});

describe("compaction stages", () => {
  const status = (s: string, fields: Record<string, unknown> = {}) =>
    ({ type: "system", subtype: "status", status: s, ...fields }) as RawEvent;
  const ended = status(null as unknown as string, { compact_result: "success" });
  const setStatus = (store: ReturnType<typeof makeStore>, next: AgentRecord["status"]) => {
    store.setState({
      workspace: { agents: [{ ...agent, status: next }], projects: [] },
      sending: {},
    } as unknown as Partial<AppState>);
    store.getState().observeCompactionStatus("a1", next);
  };

  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("holds through /compact's own turn ending, until the compacted transcript lands", async () => {
    const store = makeStore();
    const stage = () => store.getState().compactions.a1;
    await store.getState().compactAgent("a1");
    expect(stage()).toBe("requested");

    // `/compact`'s turn closes before the compaction has said anything.
    setStatus(store, "idle");
    vi.advanceTimersByTime(COMPACTION_SETTLE_MS / 2);
    store.getState().observeCompactionEvent("a1", status("compacting"));
    expect(stage()).toBe("running");

    // Idle stretches mid-compaction don't end it, however long.
    setStatus(store, "idle");
    vi.advanceTimersByTime(COMPACTION_SETTLE_MS * 3);
    expect(stage()).toBe("running");

    store.getState().observeCompactionEvent("a1", ended);
    expect(stage()).toBe("landing");
    store.getState().observeCompactionRecords("a1");
    expect(stage()).toBeUndefined();
  });

  it("follows a compaction Claude starts on its own", () => {
    const store = makeStore();
    store.getState().observeCompactionEvent("a1", status("compacting"));
    expect(store.getState().compactions.a1).toBe("running");
    store
      .getState()
      .observeCompactionEvent("a1", { type: "system", subtype: "compact_boundary" } as RawEvent);
    expect(store.getState().compactions.a1).toBe("landing");
  });

  it("ignores a boundary or records with no compaction in flight", () => {
    const store = makeStore();
    store.getState().observeCompactionEvent("a1", ended);
    store.getState().observeCompactionRecords("a1");
    expect(store.getState().compactions.a1).toBeUndefined();
  });

  it("ends on a compaction error, and when the process goes", () => {
    const store = makeStore();
    store.getState().observeCompactionEvent("a1", status("compacting"));
    store
      .getState()
      .observeCompactionEvent("a1", status(null as unknown as string, { compact_error: "x" }));
    expect(store.getState().compactions.a1).toBeUndefined();

    store.getState().observeCompactionEvent("a1", status("compacting"));
    setStatus(store, "error");
    expect(store.getState().compactions.a1).toBeUndefined();
  });

  it("gives up on a /compact Claude never starts, once the agent has sat idle", async () => {
    const store = makeStore();
    await store.getState().compactAgent("a1");
    setStatus(store, "idle");
    vi.advanceTimersByTime(COMPACTION_SETTLE_MS - 1);
    expect(store.getState().compactions.a1).toBe("requested");
    vi.advanceTimersByTime(1);
    expect(store.getState().compactions.a1).toBeUndefined();
  });

  it("keeps a compaction Claude keeps announcing, and drops one that goes quiet", () => {
    const store = makeStore();
    const stage = () => store.getState().compactions.a1;
    store.getState().observeCompactionEvent("a1", status("compacting"));
    // Claude re-announces every 30s; each one restarts the quiet clock.
    for (let i = 0; i < 4; i++) {
      vi.advanceTimersByTime(30_000);
      store.getState().observeCompactionEvent("a1", status("compacting"));
    }
    vi.advanceTimersByTime(COMPACTION_QUIET_MS - 1);
    expect(stage()).toBe("running");
    // An end this client never saw: give up rather than say "Compacting…" forever.
    vi.advanceTimersByTime(1);
    expect(stage()).toBeUndefined();
  });

  it("doesn't give up while the agent is busy", async () => {
    const store = makeStore();
    await store.getState().compactAgent("a1");
    setStatus(store, "running");
    vi.advanceTimersByTime(COMPACTION_SETTLE_MS * 2);
    expect(store.getState().compactions.a1).toBe("requested");
  });
});
