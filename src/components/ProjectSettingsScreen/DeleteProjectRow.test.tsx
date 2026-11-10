// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { type AgentRecord, api, type Workspace } from "@/api";
import { useAppStore } from "@/store";
import { render, screen } from "@/test/render";
import { DeleteProjectRow } from "./DeleteProjectRow";

const RUNNING_NOTE = "Stop running agents before deleting this project.";

const workspaceWith = (...agents: Partial<AgentRecord>[]) => ({ agents }) as Workspace;

describe("DeleteProjectRow", () => {
  beforeEach(() => {
    useAppStore.setState(useAppStore.getInitialState(), true);
    vi.spyOn(api, "projectHasRunningAgents").mockResolvedValue(false);
  });

  const mount = () => render(<DeleteProjectRow projectId="p1" projectName="Acme" />);
  /** The button once the host has said nothing is running, which enables it. */
  const deleteButton = async () => {
    const button = screen.getByRole("button", { name: "Delete project" });
    await vi.waitFor(() => expect(button).toHaveProperty("disabled", false));
    return button;
  };

  it("renders while the workspace has not loaded", async () => {
    useAppStore.setState({ workspace: null });
    mount();
    await deleteButton();
    expect(screen.queryByText(RUNNING_NOTE)).toBeNull();
  });

  it("refuses while an agent of the project is running", () => {
    useAppStore.setState({
      workspace: workspaceWith({ id: "a1", project_id: "p1", status: "running" }),
    });
    mount();
    expect(screen.getByText(RUNNING_NOTE)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Delete project" })).toHaveProperty("disabled", true);
  });

  it("is not held by another project's running agent", async () => {
    useAppStore.setState({
      workspace: workspaceWith({ id: "a1", project_id: "p2", status: "running" }),
    });
    mount();
    await deleteButton();
    expect(screen.queryByText(RUNNING_NOTE)).toBeNull();
  });
});
