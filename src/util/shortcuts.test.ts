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
      projectScreen: null,
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

  it("⌘⇧, and ⌘⇧R open their project view from the other, and leave it from its own", () => {
    const open = () => useAppStore.getState().projectScreen;
    useAppStore.setState({ projectScreen: { repoPath: project.path, view: "roadmap" } });
    runShortcutAction("projectSettings");
    expect(open()).toEqual({ repoPath: project.path, view: "settings" });
    runShortcutAction("roadmap");
    expect(open()).toEqual({ repoPath: project.path, view: "roadmap" });
    runShortcutAction("roadmap");
    expect(open()).toBeNull();
  });
});
