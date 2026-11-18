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

  it("brings a new draft in, and draws one moved from another host in place", () => {
    const projects = [{ path: "/repos/acme", name: "Acme", project_id: "p1" }] as ProjectRef[];
    useAppStore.setState({ workspace: { projects, agents: [] } as unknown as Workspace });

    const fresh = render(<EmptyWorkspace draft={draft} />);
    expect(fresh.container.querySelector(".empty-wrap")?.classList.contains("fade-in")).toBe(true);
    fresh.unmount();

    const moved = render(<EmptyWorkspace draft={{ ...draft, arrived: true }} />);
    expect(moved.container.querySelector(".fade-in")).toBeNull();
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

  it("says what a copy of another device's project starts from, only on a copy", () => {
    const own = [{ path: "/repos/acme", name: "Acme", project_id: "p1" }] as ProjectRef[];
    useAppStore.setState({ workspace: { projects: own, agents: [] } as unknown as Workspace });
    const note =
      "Starts from the branch as pushed. Commits not pushed from MacBook are not included.";

    const here = render(<EmptyWorkspace draft={draft} />);
    expect(screen.queryByText(note)).toBeNull();
    here.unmount();

    const copy = [{ ...own[0], home: { device: "macbook-key", name: "MacBook" } }] as ProjectRef[];
    useAppStore.setState({ workspace: { projects: copy, agents: [] } as unknown as Workspace });
    render(<EmptyWorkspace draft={draft} />);
    expect(screen.getByText(note)).toBeTruthy();
  });

  it("shows only that it is being moved while its project is made ready elsewhere", () => {
    const projects = [{ path: "/repos/acme", name: "Acme", project_id: "p1" }] as ProjectRef[];
    useAppStore.setState({ workspace: { projects, agents: [] } as unknown as Workspace });
    render(<EmptyWorkspace draft={{ ...draft, preparing: "Preparing the project on Studio…" }} />);

    expect(screen.getByRole("status").textContent).toBe("Preparing the project on Studio…");
    // The row's pickers are out of reach until it has moved or failed.
    expect((screen.getByText("Acme").closest(".empty-meta") as HTMLElement).style.display).toBe(
      "none",
    );
  });
});
