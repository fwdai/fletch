// @vitest-environment happy-dom
// A publish waiting on an answer belongs to the engine that is blocked on it,
// whichever host is on screen: the prompt shows it, and the answer goes back
// to that engine and to no other.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn(() => Promise.resolve(undefined)) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import type { PublishApproval as Request } from "@/api";
import type { Transport } from "@/api/transport";
import { hostStore, stopHost, useAppStore } from "@/store";
import { LOCAL_ENVIRONMENT_ID } from "@/store/environments";
import { fireEvent, render, screen } from "@/test/render";
import { PublishApproval } from ".";

const request = (id: string, detail: string): Request => ({
  id,
  agent_id: "fuji",
  op: "git_push",
  detail,
});

/** The studio, recording what it is asked. */
const calls: { op: string; args?: Record<string, unknown> }[] = [];
const studio: Transport = {
  call: <T,>(op: string, args?: Record<string, unknown>) => {
    calls.push({ op, args });
    return Promise.resolve(undefined as T);
  },
  on: () => Promise.resolve(() => {}),
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
  });
});

afterEach(async () => {
  await stopHost("studio");
  useAppStore.getState().removeEnvironment("studio");
});

describe("PublishApproval", () => {
  it("shows a host's request while This Mac is on screen, and answers that host", () => {
    hostStore("studio").setState({ pendingPublishApprovals: [request("r1", "push feat/fuji")] });
    expect(useAppStore.getState().activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);

    render(<PublishApproval />);
    expect(screen.getByText(/An agent on Studio wants to/)).toBeTruthy();
    expect(screen.getByText("push feat/fuji")).toBeTruthy();

    fireEvent.click(screen.getByText("Publish"));

    expect(calls).toEqual([{ op: "answer_publish_approval", args: { id: "r1", approved: true } }]);
    expect(invoke).not.toHaveBeenCalled();
    expect(hostStore("studio").getState().pendingPublishApprovals).toEqual([]);
    expect(screen.queryByText("Publish")).toBeNull();
  });

  it("asks This Mac's first, counting every host's behind it, then the next host's", () => {
    useAppStore.setState({ pendingPublishApprovals: [request("mine", "open a PR")] });
    hostStore("studio").setState({ pendingPublishApprovals: [request("theirs", "push main")] });

    render(<PublishApproval />);
    expect(screen.getByText(/^An agent wants to/)).toBeTruthy();
    expect(screen.getByText("1 more request waiting.")).toBeTruthy();

    fireEvent.click(screen.getByText("Don't publish"));
    expect(invoke).toHaveBeenCalledWith("answer_publish_approval", { id: "mine", approved: false });
    expect(calls).toEqual([]);

    // The studio's is next, and answered there.
    expect(screen.getByText(/An agent on Studio wants to/)).toBeTruthy();
    fireEvent.click(screen.getByText("Publish"));
    expect(calls).toEqual([
      { op: "answer_publish_approval", args: { id: "theirs", approved: true } },
    ]);
  });
});
