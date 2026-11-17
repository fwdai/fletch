// @vitest-environment happy-dom
// A VM holding a connection belongs to the host it runs on, whichever host is
// on screen: its question shows, names that host, and is answered there.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn(() => Promise.resolve(undefined)) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import type { AgentRecord, EgressPrompt, Workspace } from "@/api";
import type { Transport } from "@/api/transport";
import { hostStore, stopHost, useAppStore } from "@/store";
import { GATES } from "@/store/gates";
import { fireEvent, render, screen } from "@/test/render";
import { BoxEgressToast } from "./BoxEgressToast";

const calls: { op: string; args?: Record<string, unknown> }[] = [];
const studio: Transport = {
  call: <T,>(op: string, args?: Record<string, unknown>) => {
    calls.push({ op, args });
    return Promise.resolve(undefined as T);
  },
  on: () => Promise.resolve(() => {}),
};

const held: EgressPrompt = {
  id: "e1",
  agent_id: "fuji",
  project_id: "p1",
  host: "registry.npmjs.org",
  port: 443,
  requested_at: "2026-11-17T10:00:00Z",
};

beforeEach(() => {
  calls.length = 0;
  invoke.mockClear();
  useAppStore.setState(useAppStore.getInitialState(), true);
  useAppStore.getState().upsertEnvironment({
    id: "studio",
    name: "Studio",
    kind: "remote",
    connection: "connected",
    transport: studio,
    // A host that answers the posture's ops.
    protocol: { version: 3, ops: [...GATES.egressPrompt.op], events: [], features: [] },
  });
});

afterEach(async () => {
  await stopHost("studio");
  useAppStore.getState().removeEnvironment("studio");
});

describe("BoxEgressToast", () => {
  it("asks a background host's question, named for it, and answers that host", () => {
    hostStore("studio").setState({
      workspace: {
        repos: [],
        projects: [],
        agents: [{ id: "fuji", name: "fuji", repos: [] } as unknown as AgentRecord],
      } as Workspace,
      pendingEgressPrompts: [held],
    });

    render(<BoxEgressToast />);
    expect(screen.getByText(/fuji on Studio wants to reach/)).toBeTruthy();

    fireEvent.click(screen.getByText("This project"));

    expect(calls).toEqual([{ op: "box_egress_answer", args: { id: "e1", answer: "project" } }]);
    expect(invoke).not.toHaveBeenCalledWith("box_egress_answer", expect.anything());
    expect(hostStore("studio").getState().pendingEgressPrompts).toEqual([]);
  });
});
