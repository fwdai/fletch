// The store's half of a rewind: the transcript it hands the backend, and where
// it leaves the chat, the composer and a code restore's undo afterwards — for
// each outcome the backend can come back with.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const api = vi.hoisted(() => ({
  rewindAgent: vi.fn(),
  undoCodeRestore: vi.fn(),
  readSessionRecords: vi.fn(),
  readUserTurns: vi.fn(),
  syncSession: vi.fn(),
}));
vi.mock("@/api", () => ({ api }));
vi.mock("@/pty/buffers", () => ({ clearOutputBuffer: vi.fn(), dropAgentPty: vi.fn() }));

import type { ChatItem } from "@/adapters";
import type { RestoreReport, SessionRecord, UserTurn } from "@/api";
import { createBackgroundTasksSlice } from "./backgroundTasks";
import { createComposerSlice } from "./composer";
import { createRewindSlice } from "./rewind";
import type { AppState } from "./types";
import { createWorkspaceSlice } from "./workspace";

const record = (uuid: string, role: "user" | "assistant", text: string): SessionRecord => ({
  seq: 0,
  provider: "claude",
  source: "transcript",
  native_id: uuid,
  agent_version: null,
  body:
    role === "user"
      ? { type: "user", uuid, message: { role, content: text } }
      : { type: "assistant", uuid, message: { role, content: [{ type: "text", text }] } },
});

const turn = (turnId: string, nativeId: string, text: string): UserTurn => ({
  turn_id: turnId,
  seq: 0,
  text,
  attachments: [],
  native_id: nativeId,
  started_at: 1,
  ended_at: 2,
});

/** `denali` said q0 and q1; the chat shows both. */
const HISTORY = [record("u0", "user", "q0"), record("a0", "assistant", "a0")];
const SHOWN: ChatItem[] = [
  { kind: "user_message", text: "q0", turnId: "t0" },
  { kind: "agent_message", text: "a0" },
  { kind: "user_message", text: "q1", turnId: "t1" },
];

const report: RestoreReport = {
  repos: [
    {
      subdir: "repo",
      branch: "main",
      checkpoint: "c0ffee",
      leaving: [],
      undo_ref: "refs/fletch/undo/1",
    },
  ],
};

const makeStore = () => {
  const store = create<AppState>()(
    (...a) =>
      ({
        ...createWorkspaceSlice(...a),
        ...createComposerSlice(...a),
        ...createBackgroundTasksSlice(...a),
        ...createRewindSlice(...a),
      }) as AppState,
  );
  store.setState({
    // biome-ignore lint/suspicious/noExplicitAny: minimal workspace fixture
    workspace: { repos: [], projects: [], agents: [{ id: "denali", provider: "claude" }] } as any,
    managedLogs: { denali: SHOWN },
    lastError: null,
    // biome-ignore lint/suspicious/noExplicitAny: partial store seed
  } as any);
  return store;
};

describe("rewindAgent", () => {
  beforeEach(() => {
    for (const fn of Object.values(api)) fn.mockReset();
    api.syncSession.mockResolvedValue(undefined);
    // Before the rewind, the history through q1; after it, up to q1.
    api.readSessionRecords
      .mockResolvedValueOnce([...HISTORY, record("u1", "user", "q1")])
      .mockResolvedValue(HISTORY);
    api.readUserTurns
      .mockResolvedValueOnce([turn("t0", "u0", "q0"), turn("t1", "u1", "q1")])
      .mockResolvedValue([turn("t0", "u0", "q0")]);
  });

  it("rewinds the conversation, then reloads the chat and puts the message back", async () => {
    const store = makeStore();
    api.rewindAgent.mockResolvedValue({ code: null, conversation_error: null });

    await store.getState().rewindAgent("denali", "t1", "conversation", "q1");

    // The transcript of what comes before the message, for a summary.
    expect(api.rewindAgent).toHaveBeenCalledWith(
      "denali",
      "t1",
      "conversation",
      "User: q0\n\nAssistant: a0",
    );
    const s = store.getState();
    expect(s.managedLogs.denali.map((it) => it.kind === "user_message" && it.text)).toEqual([
      "q0",
      false,
    ]);
    expect(s.composerSeeds.denali).toBe("q1");
    expect(s.codeUndo).toEqual({});
    expect(s.lastError).toBeNull();
  });

  it("keeps a code restore's undo and leaves the chat and composer alone", async () => {
    const store = makeStore();
    api.rewindAgent.mockResolvedValue({ code: report, conversation_error: null });

    await store.getState().rewindAgent("denali", "t1", "code", "q1");

    expect(api.rewindAgent).toHaveBeenCalledWith("denali", "t1", "code", null);
    expect(api.readSessionRecords).not.toHaveBeenCalled();
    const s = store.getState();
    expect(s.codeUndo.denali).toEqual(report);
    expect(s.managedLogs.denali).toBe(SHOWN);
    expect(s.composerSeeds.denali).toBeUndefined();
  });

  it("reports a conversation that couldn't follow the code, with the code's undo", async () => {
    const store = makeStore();
    const why = "The code was restored, but the conversation couldn't be rewound: busy";
    api.rewindAgent.mockResolvedValue({ code: report, conversation_error: why });

    await store.getState().rewindAgent("denali", "t1", "both", "q1");

    const s = store.getState();
    expect(s.lastError).toBe(why);
    expect(s.codeUndo.denali).toEqual(report);
    expect(s.managedLogs.denali).toBe(SHOWN);
    expect(s.composerSeeds.denali).toBeUndefined();
  });

  it("leaves everything as it was when the rewind is refused", async () => {
    const store = makeStore();
    api.rewindAgent.mockRejectedValue("Stop the agent before rewinding.");

    await store.getState().rewindAgent("denali", "t1", "both", "q1");

    const s = store.getState();
    expect(s.lastError).toBe("Stop the agent before rewinding.");
    expect(s.managedLogs.denali).toBe(SHOWN);
    expect(s.composerSeeds.denali).toBeUndefined();
    expect(s.codeUndo).toEqual({});
  });
});

describe("undoCodeRestore", () => {
  it("hands the restore back to undo it, then stops offering it", async () => {
    const store = makeStore();
    store.setState({ codeUndo: { denali: report } });
    api.undoCodeRestore.mockReset().mockResolvedValue(undefined);

    await store.getState().undoCodeRestore("denali");

    expect(api.undoCodeRestore).toHaveBeenCalledWith("denali", report);
    expect(store.getState().codeUndo).toEqual({});
  });
});
