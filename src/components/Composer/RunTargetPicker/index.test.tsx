// @vitest-environment happy-dom
// Which machine a new workspace runs on: the picker on the sheet behind the composer.
// It names machines only; the sandbox a workspace runs in on This Mac is the
// new-workspace row's (WorkspaceTypePicker).

import { beforeEach, describe, expect, it, vi } from "vitest";

const { projectDefinition } = vi.hoisted(() => ({ projectDefinition: vi.fn() }));
vi.mock("@/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api")>();
  return { ...actual, api: { projectDefinition } };
});

import type { ProjectDefinition } from "@/api";
import { useAppStore } from "@/store";
import { EngineScope } from "@/store/EngineScope";
import type { EnvironmentEntry } from "@/store/environments";
import { fireEvent, render, screen } from "@/test/render";
import { RunTargetPicker } from ".";

const DEFINITION: ProjectDefinition = {
  project_id: "p1",
  name: "App",
  home: { device: "this-mac", name: "MacBook" },
  repos: [{ id: "r1", dir: "app", origin: "https://github.com/acme/app.git", label: null }],
};

const studio = (over: Partial<EnvironmentEntry> = {}): EnvironmentEntry => ({
  id: "studio",
  name: "Studio",
  kind: "remote",
  connection: "connected",
  protocol: { version: 9, ops: ["ensure_project"], events: [], features: [] },
  ...over,
});

function picker(onPick = vi.fn(), pinned = false) {
  return render(<RunTargetPicker projectId="p1" pinned={pinned} onPick={onPick} />);
}

beforeEach(() => {
  vi.clearAllMocks();
  projectDefinition.mockResolvedValue(DEFINITION);
  useAppStore.setState(useAppStore.getInitialState(), true);
});

describe("RunTargetPicker", () => {
  it("names the machine and lists the others, with no sandbox among them", async () => {
    useAppStore.getState().upsertEnvironment(studio());
    const onPick = vi.fn();
    picker(onPick);

    const tab = screen.getByRole("button", { name: "This Mac" });
    fireEvent.click(tab);

    expect(screen.getByText("Paired hosts")).toBeTruthy();
    expect(screen.getAllByRole("menuitemradio").map((i) => i.textContent)).toEqual([
      "This Mac",
      "Studio",
    ]);
    expect(projectDefinition).toHaveBeenCalledWith("p1");

    fireEvent.click(screen.getByRole("menuitemradio", { name: "Studio" }));
    expect(onPick).toHaveBeenCalledWith("studio");
  });

  it("moves nothing when the machine it is already on is picked", () => {
    useAppStore.getState().upsertEnvironment(studio());
    const onPick = vi.fn();
    picker(onPick);
    fireEvent.click(screen.getByRole("button", { name: "This Mac" }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "This Mac" }));
    expect(onPick).not.toHaveBeenCalled();
    expect(screen.queryByRole("menuitemradio")).toBeNull();
  });

  it("shows an offline host disabled, its connection written under its name", () => {
    useAppStore.getState().upsertEnvironment(studio({ connection: "disconnected" }));
    picker();
    fireEvent.click(screen.getByRole("button", { name: "This Mac" }));
    const host = screen.getByRole("menuitemradio", { name: /^Studio/ }) as HTMLButtonElement;
    expect(host.disabled).toBe(true);
    expect(host.textContent).toBe("Studiooffline");
  });

  it("says why the project can't go to a host once it is read", async () => {
    useAppStore.getState().upsertEnvironment(studio());
    projectDefinition.mockResolvedValue({
      ...DEFINITION,
      repos: [{ ...DEFINITION.repos[0], origin: null }],
    });
    picker();

    fireEvent.click(screen.getByRole("button", { name: "This Mac" }));
    await vi.waitFor(() => {
      const host = screen.getByRole("menuitemradio", { name: /^Studio/ }) as HTMLButtonElement;
      expect(host.disabled).toBe(true);
      expect(host.textContent).toContain("app has no remote. Publish it first.");
    });
  });

  it("only says where a pinned draft runs, on This Mac and on a host", () => {
    useAppStore.getState().upsertEnvironment(studio());
    const here = picker(vi.fn(), true);
    expect(screen.queryByRole("button")).toBeNull();
    expect(here.container.textContent).toBe("This Mac");
    here.unmount();

    const there = render(
      <EngineScope hostId="studio">
        <RunTargetPicker projectId="p1" pinned onPick={vi.fn()} />
      </EngineScope>,
    );
    expect(screen.queryByRole("button")).toBeNull();
    expect(there.container.textContent).toBe("Studio");
  });
});
