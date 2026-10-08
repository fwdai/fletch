// Switching an agent's account goes through the host and the store keeps the
// stamp the host hands back: no optimistic stamp, so a refusal can never leave
// the header naming an account the agent isn't on. Status stays with the
// `agent:*` events, and a send right after a switch runs under the new account.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { switchAgentAccount, sendUserMessage } = vi.hoisted(() => ({
  switchAgentAccount: vi.fn(),
  sendUserMessage: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { switchAgentAccount, sendUserMessage } }));
vi.mock("@/pty/buffers", () => ({ clearOutputBuffer: vi.fn(), dropAgentPty: vi.fn() }));

import type { AgentRecord } from "@/api";
import type { AppState } from "./types";
import { createWorkspaceSlice } from "./workspace";

const agent = (id: string, account: string | null = null, status: AgentRecord["status"] = "idle") =>
  ({ id, provider: "claude", status, account, repos: [] }) as unknown as AgentRecord;

const makeStore = (agents: AgentRecord[], offSidebar: Record<string, AgentRecord> = {}) => {
  const store = create<AppState>()((...a) => ({ ...createWorkspaceSlice(...a) }) as AppState);
  store.setState({
    // biome-ignore lint/suspicious/noExplicitAny: minimal workspace fixture
    workspace: { agents } as any,
    offSidebarAgents: offSidebar,
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

/** A promise the test settles by hand, to hold a switch in flight. */
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

describe("switchAgentAccount", () => {
  beforeEach(() => {
    switchAgentAccount.mockReset();
    sendUserMessage.mockReset().mockResolvedValue(false);
  });

  it("sends the agent id and the target account to the host", async () => {
    switchAgentAccount.mockResolvedValue(agent("a", "work"));
    const store = makeStore([agent("a")]);

    await store.getState().switchAgentAccount("a", "work");

    expect(switchAgentAccount).toHaveBeenCalledWith("a", "work");
  });

  it("applies the stamp the host returns", async () => {
    switchAgentAccount.mockResolvedValue(agent("a", "work"));
    const store = makeStore([agent("a"), agent("b")]);

    const record = await store.getState().switchAgentAccount("a", "work");

    expect(record?.account).toBe("work");
    expect(recordOf(store, "a")?.account).toBe("work");
    expect(recordOf(store, "b")?.account).toBeNull();
  });

  it("applies the returned stamp to an off-sidebar chat too", async () => {
    switchAgentAccount.mockResolvedValue(agent("pm", "work"));
    const store = makeStore([], { pm: agent("pm") });

    await store.getState().switchAgentAccount("pm", "work");

    expect(store.getState().offSidebarAgents.pm.account).toBe("work");
  });

  it("keeps a status event that landed before the host's response", async () => {
    const pending = deferred<AgentRecord>();
    switchAgentAccount.mockReturnValue(pending.promise);
    const store = makeStore([agent("a")]);

    const switching = store.getState().switchAgentAccount("a", "work");
    store.setState({ workspace: { agents: [agent("a", null, "running")] } as never });
    pending.resolve(agent("a", "work", "spawning"));
    await switching;

    expect(recordOf(store, "a")?.status).toBe("running");
    expect(recordOf(store, "a")?.account).toBe("work");
  });

  it("leaves the stamp alone and surfaces the host's reason when refused", async () => {
    switchAgentAccount.mockRejectedValue("Wait for the turn to finish before switching accounts.");
    const store = makeStore([agent("a", "work")]);

    const record = await store.getState().switchAgentAccount("a", "default");

    expect(record).toBeNull();
    expect(recordOf(store, "a")?.account).toBe("work");
    expect(store.getState().lastError).toBe(
      "Wait for the turn to finish before switching accounts.",
    );
  });

  it("marks the switch in flight until the host answers", async () => {
    const pending = deferred<AgentRecord>();
    switchAgentAccount.mockReturnValue(pending.promise);
    const store = makeStore([agent("a")]);

    const switching = store.getState().switchAgentAccount("a", "work");
    expect(store.getState().switchingAccount.a).toBe(true);
    pending.resolve(agent("a", "work"));
    await switching;

    expect(store.getState().switchingAccount.a).toBe(false);
  });

  it("refuses a second switch while one is in flight", async () => {
    const pending = deferred<AgentRecord>();
    switchAgentAccount.mockReturnValue(pending.promise);
    const store = makeStore([agent("a")]);

    const first = store.getState().switchAgentAccount("a", "work");
    const second = await store.getState().switchAgentAccount("a", "spare");
    pending.resolve(agent("a", "work"));
    await first;

    expect(second).toBeNull();
    expect(switchAgentAccount).toHaveBeenCalledTimes(1);
  });

  it("holds a send issued right after a switch until the switch lands", async () => {
    const pending = deferred<AgentRecord>();
    switchAgentAccount.mockReturnValue(pending.promise);
    const store = makeStore([agent("a")]);

    const switching = store.getState().switchAgentAccount("a", "work");
    const sending = store.getState().sendUserMessage("a", "carry on");
    await flush();
    expect(sendUserMessage).not.toHaveBeenCalled();

    pending.resolve(agent("a", "work"));
    await Promise.all([switching, sending]);
    expect(sendUserMessage).toHaveBeenCalledTimes(1);
  });

  it("lets the next send through after a refused switch", async () => {
    const pending = deferred<AgentRecord>();
    switchAgentAccount.mockReturnValue(pending.promise);
    const store = makeStore([agent("a")]);

    const switching = store.getState().switchAgentAccount("a", "work");
    const sending = store.getState().sendUserMessage("a", "carry on");
    pending.reject("No claude account named `work`.");
    await Promise.all([switching, sending]);

    expect(sendUserMessage).toHaveBeenCalledTimes(1);
  });
});
