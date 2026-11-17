// @vitest-environment happy-dom
// A VM holding a connection belongs to the host it runs on, whichever host is
// on screen: its question shows, names that host, and is answered there.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn(() => Promise.resolve(undefined)) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import type { AgentRecord, EgressPrompt, ProjectFileStatus, Workspace } from "@/api";
import type { Transport } from "@/api/transport";
import { hostStore, stopHost, useAppStore } from "@/store";
import { GATES } from "@/store/gates";
import { fireEvent, render, screen } from "@/test/render";
import { BoxEgressToast } from "./BoxEgressToast";

/** A project whose settings file asks for a host nobody has approved. */
const asking: ProjectFileStatus = {
  project_id: "p1",
  repo_path: "/code/fletch",
  error: null,
  files: [],
  egress: { applied: [], pending: ["example.com"], disabled: [] },
  seed: {} as ProjectFileStatus["seed"],
};
/** Either host with that project in its list. */
const withProject = {
  repos: [],
  projects: [{ project_id: "p1", name: "fletch", path: "/code/fletch" }],
  agents: [],
} as unknown as Workspace;

const calls: { op: string; args?: Record<string, unknown> }[] = [];
const studio: Transport = {
  call: <T,>(op: string, args?: Record<string, unknown>) => {
    calls.push({ op, args });
    return Promise.resolve((op === "project_file_status" ? asking : undefined) as T);
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
  invoke.mockReset();
  invoke.mockImplementation(() => Promise.resolve(undefined));
  useAppStore.setState(useAppStore.getInitialState(), true);
  useAppStore.getState().upsertEnvironment({
    id: "studio",
    name: "Studio",
    kind: "remote",
    connection: "connected",
    transport: studio,
    // A host that answers the posture's ops.
    protocol: {
      version: 3,
      ops: [...GATES.egressPrompt.op, ...GATES.projectFile.op],
      events: [],
      features: [],
    },
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

  it("tells two hosts' settings file prompts for one project apart", async () => {
    invoke.mockImplementation(((op: string) =>
      Promise.resolve(op === "project_file_status" ? asking : undefined)) as typeof invoke);
    useAppStore.setState({ workspace: withProject });
    hostStore("studio").setState({ workspace: withProject });

    render(<BoxEgressToast />);

    expect(await screen.findByText("fletch\u2019s settings file asks to allow a host")).toBeTruthy();
    expect(
      await screen.findByText("fletch\u2019s settings file on Studio asks to allow a host"),
    ).toBeTruthy();
  });
});
