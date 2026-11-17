import { beforeEach, describe, expect, it, vi } from "vitest";
import { hostStore, useAppStore } from "@/store";
import { LOCAL_ENVIRONMENT_ID } from "@/store/environments";
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

  it("⌘N over a paired host opens the draft in This Mac's scope for a project This Mac has", () => {
    const copy = { ...project, path: "/copies/p1/fletch" };
    useAppStore.getState().upsertEnvironment({
      id: "studio",
      name: "Studio",
      kind: "remote",
      connection: "connected",
    });
    hostStore("studio").setState({
      workspace: { repos: [copy.path], projects: [copy], agents: [] },
    });
    useAppStore.setState({ activeEnvironmentId: "studio", lastRepoPath: copy.path });

    runShortcutAction("newAgent");

    const s = useAppStore.getState();
    expect(s.activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);
    expect(s.createDraft).toHaveBeenCalledWith(project.path);
    s.removeEnvironment("studio");
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
