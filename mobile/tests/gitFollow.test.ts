import type { AgentRecord, Workspace } from "@desktop/api/types/agent";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { gitFollowMs, useGitFollow } from "../src/screens/Agent/useGitFollow";
import { useStore } from "../src/store";

describe("gitFollowMs", () => {
  it("reads briskly behind Changes and Ship, slowly behind the chat", () => {
    expect(gitFollowMs("changes")).toBe(3_000);
    expect(gitFollowMs("ship")).toBe(3_000);
    expect(gitFollowMs("chat")).toBe(10_000);
  });
});

describe("useGitFollow", () => {
  let host: HTMLDivElement;
  let root: Root;
  const loadGitState = vi.fn(async (_agentId: string) => {});

  const agent = (over: Partial<AgentRecord> = {}) =>
    ({ id: "a", name: "Audit", status: "idle", repos: [], ...over }) as unknown as AgentRecord;

  const Probe = ({ pane }: { pane: string }) => {
    useGitFollow("a", pane);
    return null;
  };
  const render = (pane: string) => act(async () => root.render(createElement(Probe, { pane })));

  beforeEach(() => {
    vi.useFakeTimers();
    loadGitState.mockClear();
    useStore.setState({
      loadGitState,
      connection: "connected",
      workspace: { projects: [], agents: [agent()] } as unknown as Workspace,
    });
    host = document.createElement("div");
    document.body.append(host);
    root = createRoot(host);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    host.remove();
    vi.useRealTimers();
  });

  it("reads at once, then on the pane's interval, and again on switching panes", async () => {
    await render("chat");
    expect(loadGitState).toHaveBeenCalledTimes(1);
    expect(loadGitState).toHaveBeenCalledWith("a");
    vi.advanceTimersByTime(10_000);
    expect(loadGitState).toHaveBeenCalledTimes(2);

    await render("changes");
    expect(loadGitState).toHaveBeenCalledTimes(3);
    vi.advanceTimersByTime(3_000);
    expect(loadGitState).toHaveBeenCalledTimes(4);
  });

  it("reads nothing while disconnected or for a planning chat", async () => {
    useStore.setState({ connection: "connecting" });
    await render("changes");
    vi.advanceTimersByTime(30_000);

    useStore.setState({
      connection: "connected",
      workspace: { projects: [], agents: [agent({ purpose: "pm" })] } as unknown as Workspace,
    });
    await render("chat");
    vi.advanceTimersByTime(30_000);
    expect(loadGitState).not.toHaveBeenCalled();
  });
});
