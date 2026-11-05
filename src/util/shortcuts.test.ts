import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "@/store";
import { runShortcutAction } from "./shortcuts";

vi.mock("@/util/platform", () => ({ IS_MAC: true, IS_WINDOWS: false }));

// The actions read the live store, so these set just what each one looks at.
const project = { path: "/w/fletch", name: "fletch", project_id: "p1", label: null };

describe("global shortcut actions", () => {
  beforeEach(() => {
    useAppStore.setState({
      workspace: { repos: [project.path], projects: [project], agents: [] },
      lastRepoPath: project.path,
      settingsScreenOpen: false,
      usageScreenOpen: false,
      projectScreenRepoPath: null,
      projectScreenTab: "roadmap",
      historyOpen: false,
      paletteOpen: false,
      createDraft: vi.fn(async () => "d1"),
    });
  });

  it("⌘N leaves Settings before starting the draft, so the composer is in front", () => {
    useAppStore.setState({ settingsScreenOpen: true, paletteOpen: true });
    runShortcutAction("newAgent");
    const s = useAppStore.getState();
    expect(s.settingsScreenOpen).toBe(false);
    expect(s.paletteOpen).toBe(false);
    expect(s.createDraft).toHaveBeenCalledWith(project.path);
  });

  it("⌘⇧, brings the project page's Settings tab forward, and only then toggles the page away", () => {
    useAppStore.setState({ projectScreenRepoPath: project.path, projectScreenTab: "roadmap" });
    runShortcutAction("projectSettings");
    expect(useAppStore.getState().projectScreenRepoPath).toBe(project.path);
    expect(useAppStore.getState().projectScreenTab).toBe("settings");
    runShortcutAction("projectSettings");
    expect(useAppStore.getState().projectScreenRepoPath).toBeNull();
  });
});
