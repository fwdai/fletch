// Moving a workspace to the other sandbox goes through the host, and the store
// keeps the stamp the host hands back: a refusal never leaves the title bar
// naming a sandbox the workspace isn't in. A send right after a move waits for
// it, so it reaches the moved agent.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { moveWorkspace, sendUserMessage } = vi.hoisted(() => ({
  moveWorkspace: vi.fn(),
  sendUserMessage: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { moveWorkspace, sendUserMessage } }));
vi.mock("@/pty/buffers", () => ({ clearOutputBuffer: vi.fn(), dropAgentPty: vi.fn() }));

import type { AgentRecord } from "@/api";
import type { AppState } from "./types";
import { createWorkspaceSlice } from "./workspace";

const agent = (id: string, engine: string | null = "box") =>
  ({
    id,
    provider: "claude",
    status: "idle",
    sandbox_engine: engine,
    repos: [],
  }) as unknown as AgentRecord;

const makeStore = (agents: AgentRecord[]) => {
  const store = create<AppState>()((...a) => ({ ...createWorkspaceSlice(...a) }) as AppState);
  store.setState({
    // biome-ignore lint/suspicious/noExplicitAny: minimal workspace fixture
    workspace: { agents } as any,
    offSidebarAgents: {},
    managedLogs: {},
    sending: {},
    busyLabel: {},
    lastError: null,
    // biome-ignore lint/suspicious/noExplicitAny: partial store seed
  } as any);
  return store;
};

const recordOf = (store: ReturnType<typeof makeStore>, id: string) =>
  store.getState().workspace?.agents.find((a) => a.id === id);

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("moveWorkspace", () => {
  beforeEach(() => {
    moveWorkspace.mockReset();
    sendUserMessage.mockReset().mockResolvedValue(false);
  });

  it("asks the host and applies the stamp it returns", async () => {
    moveWorkspace.mockResolvedValue(agent("a", "sandbox-exec"));
    const store = makeStore([agent("a"), agent("b")]);

    await store.getState().moveWorkspace("a", "host");

    expect(moveWorkspace).toHaveBeenCalledWith("a", "host");
    expect(recordOf(store, "a")?.sandbox_engine).toBe("sandbox-exec");
    expect(recordOf(store, "b")?.sandbox_engine).toBe("box");
  });

  it("leaves the stamp alone and surfaces the host's reason when refused", async () => {
    moveWorkspace.mockRejectedValue(
      "The Linux VM is still being set up. Try again once it's ready.",
    );
    const store = makeStore([agent("a", "sandbox-exec")]);

    const record = await store.getState().moveWorkspace("a", "box");

    expect(record).toBeNull();
    expect(recordOf(store, "a")?.sandbox_engine).toBe("sandbox-exec");
    expect(store.getState().lastError).toBe(
      "The Linux VM is still being set up. Try again once it's ready.",
    );
  });

  it("marks the move in flight, refuses a second, and holds a send until it lands", async () => {
    const pending = deferred<AgentRecord>();
    moveWorkspace.mockReturnValue(pending.promise);
    const store = makeStore([agent("a")]);

    const moving = store.getState().moveWorkspace("a", "host");
    expect(store.getState().movingWorkspace.a).toBe("host");
    expect(await store.getState().moveWorkspace("a", "host")).toBeNull();
    const sending = store.getState().sendUserMessage("a", "carry on");
    await flush();
    expect(sendUserMessage).not.toHaveBeenCalled();

    pending.resolve(agent("a", "sandbox-exec"));
    await Promise.all([moving, sending]);
    expect(store.getState().movingWorkspace.a).toBeNull();
    expect(moveWorkspace).toHaveBeenCalledTimes(1);
    expect(sendUserMessage).toHaveBeenCalledTimes(1);
  });

  it("keeps where the move is going when the host restamps the record mid-move", async () => {
    const pending = deferred<AgentRecord>();
    moveWorkspace.mockReturnValue(pending.promise);
    const store = makeStore([agent("a", "sandbox-exec")]);

    const moving = store.getState().moveWorkspace("a", "box");
    // The host restamps, then refreshes the workspace, before it answers.
    store.setState({ workspace: { agents: [agent("a", "box")] } as never });
    expect(store.getState().movingWorkspace.a).toBe("box");

    pending.resolve(agent("a", "box"));
    await moving;
    expect(store.getState().movingWorkspace.a).toBeNull();
  });
});
