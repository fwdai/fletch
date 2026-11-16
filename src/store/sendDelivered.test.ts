// `sendUserMessage` resolves true for every send that was not refused before
// it left — including one the host then rejected, which only raises
// `lastError`. What must follow a message that really went out (handing a
// roadmap item to the agent) hangs off `onDelivered` instead; these pin when
// it fires.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { sendUserMessage, grantAgentFolder } = vi.hoisted(() => ({
  sendUserMessage: vi.fn(),
  grantAgentFolder: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { sendUserMessage, grantAgentFolder } }));
vi.mock("@/pty/buffers", () => ({ clearOutputBuffer: vi.fn(), dropAgentPty: vi.fn() }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));
vi.mock("@/data/slashCommands", () => ({ discoverCommands: vi.fn() }));

import type { AgentRecord } from "@/api";
import type { AppState } from "./types";
import { createWorkspaceSlice } from "./workspace";

const agent = {
  id: "a1",
  status: "idle",
  provider: "claude",
  repos: [],
  folder_grants: [],
} as unknown as AgentRecord;

const makeStore = () => {
  const store = create<AppState>()((...a) => ({ ...createWorkspaceSlice(...a) }) as AppState);
  store.setState({
    workspace: { agents: [agent], projects: [] },
    managedLogs: {},
    sending: {},
    busyLabel: {},
  } as unknown as Partial<AppState>);
  return store;
};

describe("sendUserMessage onDelivered", () => {
  beforeEach(() => {
    sendUserMessage.mockReset();
    grantAgentFolder.mockReset();
  });

  it("fires once the host has taken the message", async () => {
    sendUserMessage.mockResolvedValue(false);
    const onDelivered = vi.fn();
    const ok = await makeStore().getState().sendUserMessage("a1", "hi", [], [], { onDelivered });
    expect(ok).toBe(true);
    expect(onDelivered).toHaveBeenCalledTimes(1);
  });

  it("stays silent when the host refuses the message, though the send resolves true", async () => {
    sendUserMessage.mockRejectedValue(new Error("agent is gone"));
    const onDelivered = vi.fn();
    const store = makeStore();
    const ok = await store.getState().sendUserMessage("a1", "hi", [], [], { onDelivered });
    expect(ok).toBe(true);
    expect(store.getState().lastError).toContain("agent is gone");
    expect(onDelivered).not.toHaveBeenCalled();
  });

  it("stays silent when a folder is refused and nothing is sent", async () => {
    grantAgentFolder.mockRejectedValue(new Error("not a folder"));
    const onDelivered = vi.fn();
    const ok = await makeStore()
      .getState()
      .sendUserMessage("a1", "hi", [], [{ path: "/nope", writable: false }], { onDelivered });
    expect(ok).toBe(false);
    expect(sendUserMessage).not.toHaveBeenCalled();
    expect(onDelivered).not.toHaveBeenCalled();
  });
});
