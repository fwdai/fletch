import { describe, expect, it, vi } from "vitest";
import type { AgentRecord, ProjectRef, WfRun } from "@/api";
import type { DraftAgent } from "@/store";
import { LOCAL_ENVIRONMENT_ID } from "@/store/environments";
import type { AppState } from "@/store/types";
import {
  emptySections,
  type PaletteContext,
  pageRows,
  rootItems,
  SEC,
  searchSections,
} from "./items";

vi.mock("@/util/platform", () => ({ IS_MAC: true, IS_WINDOWS: false }));

// ── fixtures ──────────────────────────────────────────────────────────────────

const project = (path: string, name = path.split("/").pop() ?? path): ProjectRef => ({
  path,
  name,
  project_id: `p-${name}`,
  label: null,
});

function agent(over: Partial<AgentRecord> & { id: string }): AgentRecord {
  return {
    project_id: "p-fletch",
    name: over.id,
    provider: "claude",
    repos: [
      { repo_path: "/w/fletch", subdir: "", branch: `feat/${over.id}`, parent_branch: "main" },
    ],
    task: `Task for ${over.id}`,
    status: "idle",
    view: "custom",
    created_at: "2026-11-05T10:00:00Z",
    ...over,
  };
}

const PROJECTS = [project("/w/fletch"), project("/w/relay")];

/** The slice of the store the builders read, with every action a spy. */
function state(over: Partial<AppState> = {}): AppState {
  const actions = {
    selectAgent: vi.fn(),
    selectDraft: vi.fn(),
    selectRun: vi.fn(),
    createDraft: vi.fn(),
    openRoadmap: vi.fn(),
    openProjectSettings: vi.fn(),
    captureToRoadmap: vi.fn(),
    openSettingsScreen: vi.fn(),
    openFeedback: vi.fn(),
    runUpdateCheck: vi.fn(),
    restore: vi.fn(),
    switchEnvironment: vi.fn(),
    delegateAction: vi.fn(),
    setAccent: vi.fn(),
    setLastError: vi.fn(),
    togglePalette: vi.fn(),
    setProjectFocus: vi.fn(),
    toggleHistory: vi.fn(),
    toggleSettings: vi.fn(),
    closeSettingsScreen: vi.fn(),
    closeProjectScreen: vi.fn(),
    closeUsageScreen: vi.fn(),
  };
  return {
    workspace: { repos: PROJECTS.map((p) => p.path), projects: PROJECTS, agents: [] },
    drafts: [],
    projectFocus: [],
    pendingToolUse: {},
    delegations: {},
    environments: {
      [LOCAL_ENVIRONMENT_ID]: {
        id: LOCAL_ENVIRONMENT_ID,
        name: "This Mac",
        kind: "local",
        connection: "connected",
      },
    },
    activeEnvironmentId: LOCAL_ENVIRONMENT_ID,
    features: {
      git: true,
      code: true,
      run: false,
      terminal: false,
      tokenUsage: true,
      sidebarActiveFirst: true,
      nativeView: false,
    },
    theme: "dark",
    accent: "copper",
    leftCollapsed: false,
    rightCollapsed: false,
    selectedAgentId: null,
    activeDraftId: null,
    selectedRunId: null,
    settingsScreenOpen: false,
    usageScreenOpen: false,
    projectScreen: null,
    contextLayerEnabled: false,
    historyOpen: false,
    onboardingOpen: false,
    lastRepoPath: "/w/relay",
    updateCheckStatus: null,
    updateReadyVersion: null,
    admin: false,
    ...actions,
    ...over,
  } as unknown as AppState;
}

function ctx(over: Partial<AppState> = {}, extra: Partial<PaletteContext> = {}): PaletteContext {
  return {
    s: state(over),
    runs: [],
    editors: [],
    developer: false,
    now: Date.parse("2026-11-05T12:00:00Z"),
    openUrl: vi.fn(),
    copyText: vi.fn(),
    ...extra,
  };
}

const ids = (c: PaletteContext) => rootItems(c).map((it) => it.id);
const find = (c: PaletteContext, id: string) => {
  const it = rootItems(c).find((x) => x.id === id);
  if (!it) throw new Error(`no row ${id}`);
  return it;
};

// ── what the root offers ──────────────────────────────────────────────────────

describe("rootItems", () => {
  it("offers every global shortcut as a row carrying its keymap id", () => {
    const rows = rootItems(ctx());
    for (const id of ["newAgent", "addProject", "home", "history", "usage", "shortcuts"]) {
      const row = rows.find((r) => r.id === id);
      expect(row, id).toBeDefined();
      expect(row?.shortcut).toBe(id);
      expect(row?.remember).toBe(true);
    }
  });

  it("names the project New agent will start in, the way ⌘N picks it", () => {
    expect(find(ctx(), "newAgent").hint).toBe("in relay");
    expect(find(ctx({ lastRepoPath: undefined }), "newAgent").hint).toBe("in fletch");
  });

  it("focuses the sidebar on the project in front, and offers the way back once it is", () => {
    const all = ctx();
    const focus = find(all, "focusProject");
    expect(focus.hint).toBe("Show only relay in the sidebar");
    expect(ids(all)).not.toContain("show-all-projects");
    focus.run?.();
    expect(all.s.setProjectFocus).toHaveBeenCalledWith(["p-relay"]);

    const narrowed = ctx({ projectFocus: ["p-relay"] });
    expect(ids(narrowed)).not.toContain("focusProject");
    const back = find(narrowed, "show-all-projects");
    expect(back.hint).toBe("The sidebar is on 1 of 2");
    back.run?.();
    expect(narrowed.s.setProjectFocus).toHaveBeenCalledWith([]);
  });

  it("has no project focus to offer with a single project", () => {
    const one = ctx({
      workspace: { repos: ["/w/fletch"], projects: [PROJECTS[0]], agents: [] },
      lastRepoPath: "/w/fletch",
    } as Partial<AppState>);
    expect(ids(one)).not.toContain("focusProject");
  });

  it("keeps agent-only actions off the list until an agent is in front", () => {
    const home = ids(ctx());
    expect(home).not.toContain("stopAgent");
    expect(home).not.toContain("panelGit");
    expect(home).not.toContain("copy-branch");

    const running = agent({ id: "fuji", status: "running" });
    const onAgent = ids(ctx({ workspace: ws([running]), selectedAgentId: "fuji" }));
    expect(onAgent).toContain("stopAgent");
    expect(onAgent).not.toContain("archiveAgent");
    expect(onAgent).toContain("panelGit");
    expect(onAgent).toContain("copy-branch");
  });

  it("offers archiving, not stopping, once the agent is idle", () => {
    const idle = agent({ id: "fuji" });
    const list = ids(ctx({ workspace: ws([idle]), selectedAgentId: "fuji" }));
    expect(list).toContain("archiveAgent");
    expect(list).not.toContain("stopAgent");
  });

  it("asks the agent to open a PR until there is one, then opens the one there is", () => {
    const fresh = agent({ id: "fuji" });
    const c1 = ctx({ workspace: ws([fresh]), selectedAgentId: "fuji" });
    const commit = find(c1, "commit-pr");
    expect(ids(c1)).not.toContain("view-pr");
    commit.run?.();
    expect(c1.s.delegateAction).toHaveBeenCalledWith("fuji", "commit-pr", { base: "main" });

    const withPr = agent({
      id: "etna",
      repos: [
        {
          repo_path: "/w/fletch",
          subdir: "",
          branch: "feat/etna",
          parent_branch: "main",
          pr_number: 41,
          pr_url: "https://github.com/x/y/pull/41",
          pr_title: "Add palette",
        },
      ],
    });
    const c2 = ctx({ workspace: ws([withPr]), selectedAgentId: "etna" });
    expect(ids(c2)).not.toContain("commit-pr");
    const view = find(c2, "view-pr");
    expect(view.hint).toBe("#41 · Add palette");
    view.run?.();
    expect(c2.openUrl).toHaveBeenCalledWith("https://github.com/x/y/pull/41");
  });

  it("holds the PR hand-off while the agent already has a delegation on the checkout", () => {
    const fresh = agent({ id: "fuji" });
    const c = ctx({
      workspace: ws([fresh]),
      selectedAgentId: "fuji",
      delegations: { fuji: { kind: "commit" } } as unknown as AppState["delegations"],
    });
    expect(find(c, "commit-pr").disabled).toMatch(/already working/);
  });

  it("drills into a project pick only when there is more than one project", () => {
    expect(find(ctx(), "new-agent-in").page).toBe("new-agent");
    expect(find(ctx(), "workflow-in").page).toBe("workflow");

    const one = ctx({ workspace: { repos: ["/w/fletch"], projects: [PROJECTS[0]], agents: [] } });
    expect(ids(one)).not.toContain("new-agent-in");
    const wf = find(one, "workflow-in");
    expect(wf.page).toBeUndefined();
    wf.run?.();
    expect(one.s.createDraft).toHaveBeenCalledWith("/w/fletch", { kind: "workflow" });
  });

  it("lists agents by attention: a question first, then working, then newest", () => {
    const agents = [
      agent({ id: "old", created_at: "2026-11-01T00:00:00Z" }),
      agent({ id: "new", created_at: "2026-11-05T00:00:00Z" }),
      agent({ id: "busy", status: "running" }),
      agent({ id: "asks", status: "running" }),
      agent({ id: "step", owner_run_id: "run-1" }),
      agent({
        id: "gone",
        archive: {
          archived_at: "2026-11-04T00:00:00Z",
          repos: [],
          diff_stats: { additions: 0, deletions: 0 },
        },
      }),
    ];
    const c = ctx({ workspace: ws(agents), pendingToolUse: { asks: { t1: "AskUserQuestion" } } });
    const rows = rootItems(c).filter((it) => it.section === SEC.agents);
    expect(rows.map((r) => r.id)).toEqual(["agent:asks", "agent:busy", "agent:new", "agent:old"]);
    expect(rows[0].meta).toBe("Waiting for input");
    expect(rows[0].leading).toEqual({ kind: "status", status: "waiting" });
    // The archived one is a History row, offered to restore.
    const restore = find(c, "restore:gone");
    expect(restore.section).toBe(SEC.history);
    expect(restore.hint).toBe("fletch · archived 1d ago");
    restore.run?.();
    expect(c.s.closeSettingsScreen).toHaveBeenCalled();
    expect(c.s.restore).toHaveBeenCalledWith("gone");
  });

  it("jumps to an agent through the one selection path, leaving every screen", () => {
    const a = agent({ id: "fuji" });
    const c = ctx({ workspace: ws([a]), settingsScreenOpen: true });
    find(c, "agent:fuji").run?.();
    expect(c.s.closeSettingsScreen).toHaveBeenCalled();
    expect(c.s.togglePalette).toHaveBeenCalledWith(false);
    expect(c.s.selectAgent).toHaveBeenCalledWith("fuji");
  });

  it("opens the project in front's settings or roadmap rather than leaving the view", () => {
    const c = ctx({ projectScreen: { repoPath: "/w/fletch", view: "settings" } });
    find(c, "projectSettings").run?.();
    expect(c.s.openProjectSettings).toHaveBeenCalledWith("/w/fletch");
    // Any takeover closes first, then the view opens: it ends open.
    expect(ranBefore(c.s.closeProjectScreen, c.s.openProjectSettings)).toBe(true);
    const roadmap = find(c, "roadmap");
    expect(roadmap.hint).toBe("for fletch");
    roadmap.run?.();
    expect(c.s.openRoadmap).toHaveBeenCalledWith("/w/fletch");
  });

  it("names every project's roadmap, and the Context view only while the layer is on", () => {
    const c = ctx();
    const row = find(c, "project:/w/relay");
    expect(row.label).toBe("Roadmap · relay");
    row.run?.();
    expect(c.s.openRoadmap).toHaveBeenCalledWith("/w/relay");

    expect(ids(c)).not.toContain("project-context");
    const on = ctx({ contextLayerEnabled: true });
    const context = find(on, "project-context");
    expect(context.label).toBe("Context · relay");
    context.run?.();
    expect(on.s.openRoadmap).toHaveBeenCalledWith("/w/relay", "context");
  });

  it("opens the current project's board to capture into its inbox", () => {
    const c = ctx({ projectScreen: { repoPath: "/w/relay", view: "settings" } });
    const row = find(c, "roadmap-capture");
    expect(row.label).toBe("Add to roadmap…");
    expect(row.hint).toContain("relay");
    row.run?.();
    expect(c.s.captureToRoadmap).toHaveBeenCalledWith("/w/relay");
    expect(ranBefore(c.s.closeProjectScreen, c.s.captureToRoadmap)).toBe(true);
  });

  it("offers every settings section, sub-tabs included, and the environments only once there are two", () => {
    const rows = rootItems(ctx());
    expect(rows.find((r) => r.id === "settings:providers")?.label).toBe("Settings › Providers");
    for (const [id, label, group] of [
      ["network", "Network", "Sandbox"],
      ["tools", "Tools", "Customize"],
      ["skills", "Skills", "Customize"],
    ]) {
      const row = rows.find((r) => r.id === `settings:${id}`);
      expect(row?.label, id).toBe(`Settings › ${label}`);
      expect(row?.hint, id).toBe(group);
    }
    expect(rows.filter((r) => r.id === "settings:agents")).toHaveLength(1);
    expect(rows.some((r) => r.section === SEC.environments)).toBe(false);
    expect(ids(ctx())).not.toContain("environment");

    const two = ctx({
      environments: {
        [LOCAL_ENVIRONMENT_ID]: {
          id: LOCAL_ENVIRONMENT_ID,
          name: "This Mac",
          kind: "local",
          connection: "connected",
        },
        h1: { id: "h1", name: "Studio", kind: "remote", connection: "connecting" },
      },
    });
    const studio = find(two, "env:h1");
    expect(studio.hint).toBe("connecting…");
    expect(find(two, `env:${LOCAL_ENVIRONMENT_ID}`).disabled).toBe("Already here");
    expect(find(two, "environment").page).toBe("environment");
  });

  it("lists workflow runs and drafts with their project", () => {
    const run = {
      id: "r1",
      name: "Ship it",
      task: "Build the thing",
      repo_path: "/w/fletch",
      status: "running",
      branch: "wf/ship",
    } as WfRun;
    const draft = {
      id: "d1",
      kind: "workflow",
      name: "kilimanjaro",
      repoPath: "/w/relay",
    } as DraftAgent;
    const c = ctx({ drafts: [draft] }, { runs: [run] });
    expect(find(c, "run:r1").hint).toBe("fletch · Build the thing");
    expect(find(c, "draft:d1").hint).toBe("Workflow draft · relay");
  });
});

// ── pages ─────────────────────────────────────────────────────────────────────

/** When a spy first ran, on vitest's global call clock. */
const firstCall = (spy: unknown): number =>
  (spy as ReturnType<typeof vi.fn>).mock.invocationCallOrder[0];
/** Whether spy `first` ran before spy `second`. */
const ranBefore = (first: unknown, second: unknown) => firstCall(first) < firstCall(second);

describe("pageRows", () => {
  it("puts the last-used project first on the new-agent page and starts a draft there", () => {
    const c = ctx();
    const rows = pageRows(c, "new-agent");
    expect(rows.map((r) => r.label)).toEqual(["relay", "fletch"]);
    expect(rows[0].meta).toBe("Last used");
    rows[1].run?.();
    expect(c.s.createDraft).toHaveBeenCalledWith("/w/fletch");
  });

  it("leaves Settings or a project page before starting a draft, so the composer is seen", () => {
    const fromSettings = ctx({ settingsScreenOpen: true });
    pageRows(fromSettings, "new-agent")[0].run?.();
    expect(fromSettings.s.closeSettingsScreen).toHaveBeenCalled();
    expect(ranBefore(fromSettings.s.closeSettingsScreen, fromSettings.s.createDraft)).toBe(true);

    const fromProject = ctx({ projectScreen: { repoPath: "/w/fletch", view: "roadmap" } });
    pageRows(fromProject, "workflow")[0].run?.();
    expect(fromProject.s.closeProjectScreen).toHaveBeenCalled();
    expect(fromProject.s.createDraft).toHaveBeenCalledWith("/w/relay", { kind: "workflow" });

    const one = ctx({
      settingsScreenOpen: true,
      workspace: { repos: ["/w/fletch"], projects: [PROJECTS[0]], agents: [] },
    });
    find(one, "workflow-in").run?.();
    expect(one.s.closeSettingsScreen).toHaveBeenCalled();
    expect(one.s.createDraft).toHaveBeenCalledWith("/w/fletch", { kind: "workflow" });
  });

  it("lists the accents with the current one marked", () => {
    const rows = pageRows(ctx({ accent: "ocean" }), "accent");
    expect(rows.find((r) => r.id === "accent:ocean")?.meta).toBe("Current");
    expect(rows[0].leading.kind).toBe("swatch");
  });

  it("drops the Settings › prefix on the settings page", () => {
    const rows = pageRows(ctx(), "settings");
    expect(rows[0].label).not.toMatch(/^Settings/);
    expect(rows.some((r) => r.id === "settings:developer")).toBe(false);
    expect(
      pageRows(ctx({}, { developer: true }), "settings").some((r) => r.id === "settings:developer"),
    ).toBe(true);
  });
});

// ── grouping ──────────────────────────────────────────────────────────────────

describe("emptySections", () => {
  it("suggests what fits the surface, then recents, then the agents worth a look", () => {
    const agents = [agent({ id: "fuji", status: "running" }), agent({ id: "etna" })];
    const c = ctx({ workspace: ws(agents), selectedAgentId: "fuji" });
    const sections = emptySections(c.s, rootItems(c), ["history", "stopAgent", "usage"]);
    expect(sections.map((s) => s.label)).toEqual([SEC.suggested, SEC.recent, SEC.jump]);
    const [suggested, recent, jump] = sections.map((s) => s.rows.map((r) => r.item.id));
    expect(suggested[0]).toBe("stopAgent");
    expect(suggested).toContain("commit-pr");
    // A recent already suggested is not repeated.
    expect(recent).toEqual(["history", "usage"]);
    // The selected agent is where the user already is.
    expect(jump).toEqual(["agent:etna"]);
  });

  it("leaves out a section with nothing in it", () => {
    const c = ctx();
    const sections = emptySections(c.s, rootItems(c), []);
    expect(sections.map((s) => s.label)).toEqual([SEC.suggested]);
    expect(sections[0].rows[0].item.id).toBe("newAgent");
  });
});

describe("searchSections", () => {
  it("groups the matches by section, best section first, and highlights the label", () => {
    const c = ctx({ workspace: ws([agent({ id: "fuji", title: "Git history view" })]) });
    const sections = searchSections(rootItems(c), "git");
    // The title that starts with the query leads; the section a mere
    // word-start match lives in comes after it.
    expect(sections[0].label).toBe(SEC.agents);
    expect(sections[0].rows[0].indices).toEqual([0, 1, 2]);
    const settings = sections.find((s) => s.label === SEC.settings);
    expect(settings?.rows[0].item.id).toBe("settings:git");
    expect(sections.indexOf(settings as (typeof sections)[number])).toBeGreaterThan(0);
  });

  it("caps each section while searching and shows everything on an empty query", () => {
    const many = Array.from({ length: 12 }, (_, i) => agent({ id: `a${i}`, title: `Agent ${i}` }));
    const items = rootItems(ctx({ workspace: ws(many) }));
    const searching = searchSections(items, "agent").find((s) => s.label === SEC.agents);
    expect(searching?.rows).toHaveLength(8);
    const all = searchSections(items, "").find((s) => s.label === SEC.agents);
    expect(all?.rows).toHaveLength(12);
  });

  it("finds a row by its keywords alone", () => {
    const sections = searchSections(rootItems(ctx()), "hotkeys");
    expect(sections.flatMap((s) => s.rows.map((r) => r.item.id))).toContain("shortcuts");
  });
});

function ws(agents: AgentRecord[]) {
  return { repos: PROJECTS.map((p) => p.path), projects: PROJECTS, agents };
}
