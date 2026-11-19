// @vitest-environment happy-dom
// The workspace popover for a box at a cloud provider: named for the provider
// and its bill, stopped like any box, and never offered a move, which would
// leave the cloud. A box on this Mac keeps its move.

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentRecord, BoxRuntimeProbe, CloudProviderStatus } from "@/api";
import { useAppStore } from "@/store";
import { render, screen } from "@/test/render";
import { WorkspacePopover } from "./WorkspacePopover";

// The agent's brand icon is a fetched SVG, which has nothing to say here.
vi.mock("@/components/ProviderIcon", () => ({ ProviderIcon: () => null }));

const FLY: CloudProviderStatus = {
  id: "fly",
  label: "Fly.io",
  connected: true,
  account: "acme",
  account_label: "Organization",
  account_placeholder: "personal",
  token_placeholder: "FlyV1 fm2_…",
  token_help: "Run fly tokens create org in a terminal.",
  billing: "Fly.io bills its disk while the workspace exists.",
};

const agent = (over: Partial<AgentRecord> = {}): AgentRecord => ({
  id: "a1",
  project_id: "p1",
  name: "moravia",
  provider: "claude",
  repos: [{ repo_path: "/repos/acme", subdir: "" }],
  task: "",
  status: "idle",
  view: "custom",
  created_at: "2026-11-19T00:00:00Z",
  sandbox_engine: "box",
  ...over,
});

const inCloud = agent({ cloud_provider: "fly" });

function seed(phase: "running" | "failed") {
  useAppStore.setState({
    cloudProviders: [FLY],
    boxStates: { a1: { agent_id: "a1", phase, reason: phase === "failed" ? "no tools" : null } },
    boxTools: { claude: { provider: "claude", version: "1", phase: "failed" } },
    boxProbe: { runtime_ok: true } as BoxRuntimeProbe,
  });
}

beforeEach(() => useAppStore.setState(useAppStore.getInitialState(), true));

describe("WorkspacePopover", () => {
  it("names a cloud workspace's provider and bill, and stops its box", () => {
    seed("running");
    render(<WorkspacePopover agent={inCloud} status="idle" />);
    expect(screen.getByText("Machine at Fly.io")).toBeTruthy();
    expect(
      screen.getByText(
        "Its own machine; reaches any host. Fly.io bills its disk while the workspace exists.",
      ),
    ).toBeTruthy();
    expect(screen.getByText("Running")).toBeTruthy();
    expect(screen.getByRole("button", { name: /Stop box/ })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Move to/ })).toBeNull();
  });

  it("offers a failed cloud box no rebuild of this Mac's tools disk", () => {
    seed("failed");
    render(<WorkspacePopover agent={inCloud} status="idle" />);
    expect(screen.getByText("no tools")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Retry tools install/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Move to/ })).toBeNull();
  });

  it("keeps a box on this Mac as it was", () => {
    seed("failed");
    render(<WorkspacePopover agent={agent()} status="idle" />);
    expect(screen.getByText("Linux VM")).toBeTruthy();
    expect(screen.getByRole("button", { name: /Retry tools install/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /Move to macOS/ })).toBeTruthy();
  });
});
