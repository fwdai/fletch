// Switching an agent's account goes through the host and the store keeps
// whatever record the host hands back: no optimistic stamp, so a refusal can
// never leave the header naming an account the agent isn't on.

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

const agent = (id: string, account: string | null = null) =>
  ({ id, provider: "claude", status: "idle", account }) as AgentRecord;

const makeStore = (agents: AgentRecord[], offSidebar: Record<string, AgentRecord> = {}) => {
  const store = create<AppState>()((...a) => ({ ...createWorkspaceSlice(...a) }) as AppState);
  store.setState({
    // biome-ignore lint/suspicious/noExplicitAny: minimal workspace fixture
    workspace: { agents } as any,
    offSidebarAgents: offSidebar,
    lastError: null,
    // biome-ignore lint/suspicious/noExplicitAny: partial store seed
  } as any);
  return store;
};

const accountOf = (store: ReturnType<typeof makeStore>, id: string) =>
  store.getState().workspace?.agents.find((a) => a.id === id)?.account;

describe("switchAgentAccount", () => {
  beforeEach(() => {
    switchAgentAccount.mockReset();
  });

  it("sends the agent id and the target account to the host", async () => {
    switchAgentAccount.mockResolvedValue(agent("a", "work"));
    const store = makeStore([agent("a")]);

    await store.getState().switchAgentAccount("a", "work");

    expect(switchAgentAccount).toHaveBeenCalledWith("a", "work");
  });

  it("applies the record the host returns", async () => {
    switchAgentAccount.mockResolvedValue(agent("a", "work"));
    const store = makeStore([agent("a"), agent("b")]);

    const record = await store.getState().switchAgentAccount("a", "work");

    expect(record?.account).toBe("work");
    expect(accountOf(store, "a")).toBe("work");
    expect(accountOf(store, "b")).toBeNull();
  });

  it("applies the returned record to an off-sidebar chat too", async () => {
    switchAgentAccount.mockResolvedValue(agent("pm", "work"));
    const store = makeStore([], { pm: agent("pm") });

    await store.getState().switchAgentAccount("pm", "work");

    expect(store.getState().offSidebarAgents.pm.account).toBe("work");
  });

  it("leaves the stamp alone and surfaces the host's reason when refused", async () => {
    switchAgentAccount.mockRejectedValue("Wait for the turn to finish before switching accounts");
    const store = makeStore([agent("a", "work")]);

    const record = await store.getState().switchAgentAccount("a", "default");

    expect(record).toBeNull();
    expect(accountOf(store, "a")).toBe("work");
    expect(store.getState().lastError).toBe(
      "Wait for the turn to finish before switching accounts",
    );
  });
});
