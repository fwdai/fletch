// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProjectRef, Workspace } from "@/api";
import { type DraftAgent, useAppStore } from "@/store";
import { render, screen } from "@/test/render";
import { EmptyWorkspace } from "./EmptyWorkspace";

// The composer is its own tree, and listens to the Tauri webview for drops.
vi.mock("@/components/Composer", () => ({ Composer: () => null }));

const draft: DraftAgent = {
  id: "d1",
  kind: "agent",
  repoPath: "/repos/acme",
  name: "moravia",
  provider: "claude",
  base: "main",
};

describe("EmptyWorkspace", () => {
  beforeEach(() => useAppStore.setState(useAppStore.getInitialState(), true));

  it("renders a draft while the workspace has not loaded", () => {
    useAppStore.setState({ workspace: null });
    render(<EmptyWorkspace draft={draft} />);
    expect(screen.getByText("What should be the first task?")).toBeTruthy();
  });

  it("offers the workspace's projects", () => {
    const projects = [
      { path: "/repos/acme", name: "Acme", project_id: "p1" },
      { path: "/repos/other", name: "Other", project_id: "p2" },
    ] as ProjectRef[];
    useAppStore.setState({ workspace: { projects, agents: [] } as unknown as Workspace });
    render(<EmptyWorkspace draft={draft} />);
    expect(screen.getByText("Acme")).toBeTruthy();
  });
});
