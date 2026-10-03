// A Roadmap chat is absent from the workspace snapshot, so the only fresh read
// of its record is the project's chat list. Registering that list is therefore
// where its `sending` bridge settles — the same rule a snapshot applies to a
// sidebar agent (see helpers/sending).

import { describe, expect, it, vi } from "vitest";
import { create } from "zustand";

vi.mock("@/api", () => ({ api: {} }));
vi.mock("@/pty/buffers", () => ({ clearOutputBuffer: vi.fn(), dropAgentPty: vi.fn() }));

import type { AgentRecord } from "@/api";
import type { AppState } from "./types";
import { createWorkspaceSlice } from "./workspace";

const chat = (id: string, status: AgentRecord["status"]): AgentRecord =>
  ({ id, status, provider: "claude", repos: [], purpose: "roadmap_pm" }) as unknown as AgentRecord;

const makeStore = () => {
  const store = create<AppState>()((...a) => ({ ...createWorkspaceSlice(...a) }) as AppState);
  // biome-ignore lint/suspicious/noExplicitAny: partial store seed
  store.setState({ sending: { sakura: true, kyoto: true } } as any);
  return store;
};

describe("registerOffSidebarAgents", () => {
  it("settles the sending flag of a chat the fresh list shows at rest", () => {
    const store = makeStore();
    store.getState().registerOffSidebarAgents([chat("sakura", "idle"), chat("kyoto", "running")]);
    expect(store.getState().offSidebarAgents.sakura.status).toBe("idle");
    // At rest: the status the send produced came and went off-socket.
    expect(store.getState().sending.sakura).toBeUndefined();
    // Still working: the bridge is moot, and left alone.
    expect(store.getState().sending.kyoto).toBe(true);
  });
});
