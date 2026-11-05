// The palette's rows, built from the store. Pure: given the state and a few
// things the store does not hold (the workflow runs, the detected editors),
// it says what can be done and what can be jumped to right now — and nothing
// here renders or listens. The dialog asks this once per keystroke.
//
// Two layers, deliberately. The global shortcuts (`util/keymap.ts`) are rows
// here verbatim — same label, same chord, same action — so the palette is
// also where every shortcut is discovered. On top sit the rows a chord could
// never carry: the agents, runs and projects themselves, the drill-in pages,
// and the agent actions that need context (this PR, this branch).

import type { AgentRecord, DetectedEditor, WfRun } from "@/api";
import type { IconName } from "@/components/Icon";
import { settingsNav } from "@/components/SettingsScreen/nav";
import {
  EDITOR_PREF_KEY,
  editorsFor,
  openEditor,
} from "@/components/TitleBar/OpenInEditor/editors";
import { agentDotStatus, STATUS_LABEL } from "@/components/TitleBar/WorkspaceStatus/derive";
import { ACCENTS } from "@/data/providers";
import { connectionLabel } from "@/store/capabilities";
import type { EnvironmentEntry } from "@/store/environments";
import { ADD_PROJECT_GATES, anyGateReason, gateReason } from "@/store/gates";
import { activeSurface, surfaceAgent, surfaceRepoPath } from "@/store/surface";
import type { AppState } from "@/store/types";
import { basename, firstLine, formatAge } from "@/util/format";
import { SHORTCUT_BY_ID } from "@/util/keymap";
import { IS_MAC } from "@/util/platform";
import { closeScreens, newAgentRepoPath, runShortcutAction } from "@/util/shortcuts";
import { bestMatch } from "./match";
import type { PageId, PaletteItem, Row, Section } from "./types";

/** What the rows are built from. `s` is a snapshot of the store; the rest is
 *  what lives outside it. */
export interface PaletteContext {
  s: AppState;
  runs: readonly WfRun[];
  editors: DetectedEditor[];
  /** Whether the Developer settings section is unlocked on this build. */
  developer: boolean;
  /** The clock, for "archived 2h ago". */
  now: number;
  openUrl: (url: string) => void;
  copyText: (text: string) => void;
}

// Section labels. Command sections first, then the things to jump to.
export const SEC = {
  agent: "Current agent",
  create: "Create",
  goto: "Go to",
  view: "View",
  app: "App",
  agents: "Agents",
  drafts: "Drafts",
  runs: "Workflow runs",
  projects: "Projects",
  settings: "Settings",
  history: "History",
  environments: "Environments",
  suggested: "Suggested",
  recent: "Recent",
  jump: "Jump to",
} as const;

/** How many rows a section shows while searching; the query narrows the rest. */
const SECTION_CAP: Record<string, number> = { [SEC.agents]: 8 };
const DEFAULT_CAP = 6;
const JUMP_CAP = 6;
const SUGGESTED_CAP = 5;
const RECENT_CAP = 5;

const env = (s: AppState): EnvironmentEntry =>
  s.environments[s.activeEnvironmentId] ?? Object.values(s.environments)[0];

const projectName = (s: AppState, repoPath: string | undefined): string =>
  (repoPath && s.workspace?.projects.find((p) => p.path === repoPath)?.name) ||
  (repoPath ? basename(repoPath) : "");

/** The agent's row title, as the sidebar writes it: its own title, else the
 *  first line of the task, else the codename. */
const agentTitle = (a: AgentRecord): string => a.title || firstLine(a.task) || a.name;

/** The agents the sidebar lists: live, the user's own, not a run's step. */
function sidebarAgents(s: AppState): AgentRecord[] {
  return (s.workspace?.agents ?? []).filter((a) => !a.archive && !a.purpose && !a.owner_run_id);
}

const isWorking = (a: AgentRecord) => a.status === "running" || a.status === "spawning";
const isArchivable = (a: AgentRecord) =>
  a.status === "idle" || a.status === "stopped" || a.status === "error";

/** Agents in the order someone would want them: a question waiting first,
 *  then whatever is working, then the rest newest first. */
function byAttention(s: AppState, agents: AgentRecord[]): AgentRecord[] {
  const rank = (a: AgentRecord) => {
    const status = agentDotStatus(a.status, s.pendingToolUse[a.id]);
    return status === "waiting" ? 0 : status === "running" ? 1 : status === "error" ? 2 : 3;
  };
  return [...agents].sort((a, b) => rank(a) - rank(b) || (a.created_at < b.created_at ? 1 : -1));
}

// ── Shortcut rows ─────────────────────────────────────────────────────────────

interface ShortcutRow {
  id: string;
  icon: IconName | ((s: AppState) => IconName);
  section: string;
  /** A short phrase for the row; the keymap's description is written for the
   *  settings list and goes into the keywords instead. */
  hint?: string | ((ctx: PaletteContext) => string | undefined);
  when?: (ctx: PaletteContext) => boolean;
  disabled?: (ctx: PaletteContext) => string | null;
  keywords?: string;
}

const onAgent = (ctx: PaletteContext) => surfaceAgent(ctx.s) !== null;
const onAgentOrDraft = (ctx: PaletteContext) => {
  const kind = activeSurface(ctx.s).kind;
  return kind === "agent" || kind === "draft";
};
const hasRepos = (ctx: PaletteContext) => (ctx.s.workspace?.repos.length ?? 0) > 0;
const hasAgents = (ctx: PaletteContext) => sidebarAgents(ctx.s).length > 0;

const SHORTCUT_ROWS: ShortcutRow[] = [
  {
    id: "newAgent",
    icon: "sparkle",
    section: SEC.create,
    hint: (ctx) => {
      const repo = newAgentRepoPath(ctx.s);
      return repo ? `in ${projectName(ctx.s, repo)}` : undefined;
    },
    when: hasRepos,
    keywords: "start  draft  spawn",
  },
  {
    id: "addProject",
    icon: "folder",
    section: SEC.create,
    hint: "Open a folder, clone, or create a repository",
    disabled: (ctx) => anyGateReason(env(ctx.s), ADD_PROJECT_GATES),
    keywords: "repo  repository  clone  open folder",
  },
  {
    id: "stopAgent",
    icon: "stop",
    section: SEC.agent,
    hint: "Interrupt the current turn",
    when: (ctx) => {
      const a = surfaceAgent(ctx.s);
      return a !== null && isWorking(a);
    },
    keywords: "interrupt  cancel  halt",
  },
  {
    id: "archiveAgent",
    icon: "archive",
    section: SEC.agent,
    hint: "Restore it from History later",
    when: (ctx) => {
      const a = surfaceAgent(ctx.s);
      return a !== null && isArchivable(a);
    },
    keywords: "close  remove  done",
  },
  {
    id: "openInEditor",
    icon: "code",
    section: SEC.agent,
    hint: (ctx) => {
      const a = surfaceAgent(ctx.s);
      if (!a) return undefined;
      const editors = editorsFor(a, ctx.editors);
      const preferred = localStorage.getItem(EDITOR_PREF_KEY);
      const current = editors.find((e) => e.id === preferred) ?? editors[0];
      return current ? `in ${current.label}` : "No editor can open this checkout";
    },
    when: (ctx) => onAgent(ctx) && ctx.editors.length > 0,
    keywords: "vscode  cursor  zed  terminal  checkout  worktree",
  },
  {
    id: "focusComposer",
    icon: "edit",
    section: SEC.agent,
    hint: "Put the caret in the message box",
    when: onAgentOrDraft,
    keywords: "message  prompt  type  write",
  },
  {
    id: "panelCode",
    icon: "code",
    section: SEC.agent,
    hint: "Files, editor and the live diff",
    when: (ctx) => onAgent(ctx) && ctx.s.features.code,
    keywords: "files  diff  explorer",
  },
  {
    id: "panelGit",
    icon: "branch",
    section: SEC.agent,
    hint: "Branch, changes and the pull request",
    when: (ctx) => onAgent(ctx) && ctx.s.features.git,
    keywords: "commit  push  pr  pull request  merge",
  },
  {
    id: "panelRun",
    icon: "play",
    section: SEC.agent,
    hint: "The dev server",
    when: (ctx) => onAgent(ctx) && ctx.s.features.run,
    disabled: (ctx) => gateReason(env(ctx.s), "runScripts"),
    keywords: "dev server  start  preview",
  },
  {
    id: "panelTerminal",
    icon: "terminal",
    section: SEC.agent,
    hint: "A shell in the checkout",
    when: (ctx) => onAgent(ctx) && ctx.s.features.terminal,
    disabled: (ctx) => gateReason(env(ctx.s), "sideShell"),
    keywords: "shell  console  bash  zsh",
  },
  { id: "home", icon: "home", section: SEC.goto, hint: "Leave the current agent" },
  {
    id: "history",
    icon: "history",
    section: SEC.goto,
    hint: "Archived agents",
    keywords: "archive  archived  restore  past",
  },
  {
    id: "usage",
    icon: "gauge",
    section: SEC.goto,
    hint: "Tokens and spend",
    keywords: "tokens  cost  spend  report  limits",
  },
  {
    id: "projectSettings",
    icon: "settings",
    section: SEC.goto,
    hint: (ctx) => {
      const repo = surfaceRepoPath(ctx.s);
      return repo ? `for ${projectName(ctx.s, repo)}` : undefined;
    },
    when: (ctx) => surfaceRepoPath(ctx.s) !== undefined,
    disabled: (ctx) => gateReason(env(ctx.s), "roadmap"),
    keywords: "run command  setup  env  repo",
  },
  {
    id: "quickSettings",
    icon: "settings",
    section: SEC.goto,
    hint: "Theme, panels and providers",
    keywords: "preferences  popover",
  },
  {
    id: "shortcuts",
    icon: "keyboard",
    section: SEC.goto,
    hint: "Every key binding, and how to change them",
    keywords: "keys  bindings  hotkeys  keymap",
  },
  {
    id: "search",
    icon: "search",
    section: SEC.goto,
    hint: "Narrow the list to a name or branch",
    when: hasAgents,
    keywords: "find  filter  sidebar",
  },
  {
    id: "prevAgent",
    icon: "arrowUp",
    section: SEC.goto,
    hint: "The row above in the sidebar",
    when: hasAgents,
  },
  {
    id: "nextAgent",
    icon: "arrowDown",
    section: SEC.goto,
    hint: "The row below in the sidebar",
    when: hasAgents,
  },
  {
    id: "toggleSidebar",
    icon: "sidebarL",
    section: SEC.view,
    hint: (ctx) => (ctx.s.leftCollapsed ? "Show it" : "Hide it"),
    keywords: "left  rail  collapse  show  hide",
  },
  {
    id: "togglePanel",
    icon: "sidebarR",
    section: SEC.view,
    hint: (ctx) => (ctx.s.rightCollapsed ? "Show it" : "Hide it"),
    when: (ctx) => {
      const kind = activeSurface(ctx.s).kind;
      return kind === "agent" || kind === "run";
    },
    keywords: "right  rail  collapse  show  hide",
  },
  {
    id: "toggleTheme",
    icon: (s) => (s.theme === "dark" ? "sun" : "moon"),
    section: SEC.view,
    hint: (ctx) => (ctx.s.theme === "dark" ? "Switch to light" : "Switch to dark"),
    keywords: "dark  light  mode  appearance",
  },
];

function shortcutItems(ctx: PaletteContext): PaletteItem[] {
  const items: PaletteItem[] = [];
  for (const row of SHORTCUT_ROWS) {
    const shortcut = SHORTCUT_BY_ID[row.id];
    if (!shortcut || (row.when && !row.when(ctx))) continue;
    items.push({
      id: row.id,
      label: shortcut.label,
      hint: typeof row.hint === "function" ? row.hint(ctx) : row.hint,
      leading: { kind: "icon", name: typeof row.icon === "function" ? row.icon(ctx.s) : row.icon },
      section: row.section,
      shortcut: row.id,
      keywords: [row.keywords, shortcut.description].filter(Boolean).join("  "),
      disabled: row.disabled?.(ctx) ?? null,
      run: () => runShortcutAction(row.id),
      remember: true,
    });
  }
  return items;
}

// ── Rows a chord could not carry ──────────────────────────────────────────────

/** The current agent's context actions: its PR, its branch, its project. */
function agentItems(ctx: PaletteContext): PaletteItem[] {
  const { s } = ctx;
  const agent = surfaceAgent(s);
  if (!agent) return [];
  const primary = agent.repos[0];
  const items: PaletteItem[] = [];

  if (primary?.pr_url) {
    const title = primary.pr_title ? ` · ${primary.pr_title}` : "";
    const url = primary.pr_url;
    items.push({
      id: "view-pr",
      label: "Open pull request on GitHub",
      hint: `#${primary.pr_number}${title}`,
      leading: { kind: "icon", name: "pr" },
      section: SEC.agent,
      keywords: "github  browser  pr  review  checks",
      run: () => ctx.openUrl(url),
      remember: true,
    });
  } else if (primary?.parent_branch) {
    const base = primary.parent_branch;
    const delegated = s.delegations[agent.id] != null;
    items.push({
      id: "commit-pr",
      label: "Commit & open a pull request",
      hint: `The agent writes the message and PR, against ${base}`,
      leading: { kind: "icon", name: "pr" },
      section: SEC.agent,
      keywords: "github  ship  publish  push  branch",
      disabled: delegated
        ? "The agent is already working on this checkout"
        : gateReason(env(s), "delegateGit"),
      run: () => void s.delegateAction(agent.id, "commit-pr", { base }),
      remember: true,
    });
  }

  if (primary?.branch) {
    const branch = primary.branch;
    items.push({
      id: "copy-branch",
      label: "Copy branch name",
      hint: branch,
      leading: { kind: "icon", name: "branch" },
      section: SEC.agent,
      keywords: "clipboard  git",
      run: () => ctx.copyText(branch),
      remember: true,
    });
  }

  if (primary?.repo_path) {
    const repo = primary.repo_path;
    items.push({
      id: "open-project",
      label: "Open project",
      hint: `${projectName(s, repo)} · roadmap and activity`,
      leading: { kind: "icon", name: "map" },
      section: SEC.goto,
      keywords: "roadmap  board  activity  context",
      disabled: gateReason(env(s), "roadmap"),
      run: () => s.openProjectScreen(repo),
      remember: true,
    });
  }

  if (ctx.editors.length > 1) {
    const editors = editorsFor(agent, ctx.editors);
    if (editors.length > 1) {
      items.push({
        id: "editor",
        label: "Open in…",
        hint: editors.map((e) => e.label).join(", "),
        leading: { kind: "icon", name: "external" },
        section: SEC.agent,
        keywords: "editor  vscode  cursor  zed  terminal",
        page: "editor",
      });
    }
  }
  return items;
}

function pageItems(ctx: PaletteContext): PaletteItem[] {
  const { s } = ctx;
  const projects = s.workspace?.projects ?? [];
  const items: PaletteItem[] = [];

  if (projects.length > 1) {
    items.push({
      id: "new-agent-in",
      label: "New agent in…",
      hint: "Pick the project",
      leading: { kind: "icon", name: "sparkle" },
      section: SEC.create,
      keywords: "start  draft  spawn  project",
      page: "new-agent",
    });
  }
  if (projects.length > 0) {
    const gate = gateReason(env(s), "workflows");
    const one = projects.length === 1 ? projects[0].path : null;
    items.push({
      id: "workflow-in",
      label: one ? "Run a workflow" : "Run a workflow in…",
      hint: one ? `in ${projectName(s, one)}` : "A chain of agents on one branch",
      leading: { kind: "icon", name: "combine" },
      section: SEC.create,
      keywords: "pipeline  chain  automate  steps",
      disabled: gate,
      ...(one
        ? { run: () => void s.createDraft(one, { kind: "workflow" }), remember: true }
        : { page: "workflow" as const }),
    });
  }
  if (Object.keys(s.environments).length > 1) {
    items.push({
      id: "environment",
      label: "Switch environment…",
      hint: `Now on ${env(s).name}`,
      leading: { kind: "icon", name: "laptop" },
      section: SEC.goto,
      keywords: "host  machine  remote  mac  this mac",
      page: "environment",
    });
  }
  items.push({
    id: "settings",
    label: "All settings…",
    hint: "Account, providers, sandbox, git and more",
    leading: { kind: "icon", name: "settings" },
    section: SEC.goto,
    keywords: "preferences  configure  options",
    page: "settings",
  });
  const accent = ACCENTS.find((a) => a.id === s.accent) ?? ACCENTS[0];
  items.push({
    id: "accent",
    label: "Accent colour…",
    hint: accent.label,
    leading: { kind: "swatch", color: accent.color },
    section: SEC.view,
    keywords: "color  theme  tint  copper  rust  olive  sage  ocean  plum",
    page: "accent",
  });
  items.push({
    id: "feedback",
    label: "Send feedback",
    hint: "A note to the Fletch team",
    leading: { kind: "icon", name: "megaphone" },
    section: SEC.app,
    keywords: "bug  report  idea  contact  help",
    run: () => s.openFeedback(),
    remember: true,
  });
  items.push({
    id: "update",
    label: "Check for updates",
    hint:
      s.updateCheckStatus === "checking"
        ? "Checking…"
        : s.updateReadyVersion
          ? `v${s.updateReadyVersion} is ready to install`
          : undefined,
    leading: { kind: "icon", name: "download" },
    section: SEC.app,
    keywords: "version  upgrade  release",
    disabled: s.updateCheckStatus === "checking" ? "Already checking" : null,
    run: () => void s.runUpdateCheck(),
    remember: true,
  });
  return items;
}

// ── Things to jump to ─────────────────────────────────────────────────────────

function agentRow(ctx: PaletteContext, a: AgentRecord, section: string): PaletteItem {
  const { s } = ctx;
  const status = agentDotStatus(a.status, s.pendingToolUse[a.id]);
  const repo = a.repos[0]?.repo_path;
  const branch = a.repos[0]?.branch;
  return {
    id: `agent:${a.id}`,
    label: agentTitle(a),
    hint: [projectName(s, repo), branch].filter(Boolean).join(" · "),
    meta: STATUS_LABEL[status],
    leading: { kind: "status", status },
    section,
    keywords: [a.name, firstLine(a.task, 120), branch, projectName(s, repo)]
      .filter(Boolean)
      .join("  "),
    run: () => {
      closeScreens(s);
      s.selectAgent(a.id);
    },
  };
}

function entityItems(ctx: PaletteContext): PaletteItem[] {
  const { s } = ctx;
  const items: PaletteItem[] = [];
  const active = env(s);

  for (const a of byAttention(s, sidebarAgents(s))) items.push(agentRow(ctx, a, SEC.agents));

  for (const d of s.drafts) {
    if (!s.workspace?.projects.some((p) => p.path === d.repoPath)) continue;
    items.push({
      id: `draft:${d.id}`,
      label: d.name,
      hint: `${d.kind === "workflow" ? "Workflow" : "Agent"} draft · ${projectName(s, d.repoPath)}`,
      leading: { kind: "icon", name: d.kind === "workflow" ? "combine" : "edit" },
      section: SEC.drafts,
      keywords: "draft  new  unsent",
      run: () => {
        closeScreens(s);
        s.selectDraft(d.id);
      },
    });
  }

  for (const r of ctx.runs) {
    items.push({
      id: `run:${r.id}`,
      label: r.name,
      hint: [projectName(s, r.repo_path), firstLine(r.task)].filter(Boolean).join(" · "),
      meta: r.status,
      leading: { kind: "icon", name: "combine" },
      section: SEC.runs,
      keywords: `workflow  run  ${r.branch}  ${r.status}`,
      run: () => {
        closeScreens(s);
        s.selectRun(r.id);
      },
    });
  }

  const roadmapGate = gateReason(active, "roadmap");
  for (const p of s.workspace?.projects ?? []) {
    items.push({
      id: `project:${p.path}`,
      label: p.name,
      hint: p.path,
      leading: { kind: "icon", name: "folder" },
      section: SEC.projects,
      keywords: `project  repo  roadmap  ${basename(p.path)}`,
      disabled: roadmapGate,
      run: () => s.openProjectScreen(p.path),
    });
  }

  const nav = settingsNav({
    workflows: !gateReason(active, "workflows"),
    mac: IS_MAC,
    developer: ctx.developer,
  });
  for (const n of nav) {
    items.push({
      id: `settings:${n.id}`,
      label: `Settings › ${n.label}`,
      hint: n.group,
      leading: { kind: "icon", name: n.icon },
      section: SEC.settings,
      keywords: `settings  preferences  ${n.id}`,
      run: () => s.openSettingsScreen(n.id),
    });
  }

  const restoreGate = gateReason(active, "restore");
  const archived = (s.workspace?.agents ?? [])
    .filter((a) => a.archive)
    .sort((a, b) => ((a.archive?.archived_at ?? "") < (b.archive?.archived_at ?? "") ? 1 : -1));
  for (const a of archived) {
    const age = a.archive ? formatAge(a.archive.archived_at, ctx.now) : "";
    const when = age === "now" ? "just now" : age ? `${age} ago` : "";
    items.push({
      id: `restore:${a.id}`,
      label: agentTitle(a),
      hint: [projectName(s, a.repos[0]?.repo_path), when && `archived ${when}`]
        .filter(Boolean)
        .join(" · "),
      meta: "Restore",
      leading: { kind: "icon", name: "archiveRestore" },
      section: SEC.history,
      keywords: [a.name, firstLine(a.task, 120), a.repos[0]?.branch, "archived  history  restore"]
        .filter(Boolean)
        .join("  "),
      disabled: restoreGate,
      run: () => void s.restore(a.id),
    });
  }

  const envs = Object.values(s.environments);
  if (envs.length > 1) {
    for (const e of envs) items.push(environmentRow(s, e, SEC.environments));
  }
  return items;
}

function environmentRow(s: AppState, e: EnvironmentEntry, section: string): PaletteItem {
  const current = e.id === s.activeEnvironmentId;
  return {
    id: `env:${e.id}`,
    label: e.name,
    hint: connectionLabel(e),
    meta: current ? "Current" : undefined,
    leading: { kind: "icon", name: e.kind === "local" ? "laptop" : "globe" },
    section,
    keywords: "environment  host  switch  machine",
    disabled: current ? "Already here" : null,
    run: () => void s.switchEnvironment(e.id),
  };
}

// ── Pages ─────────────────────────────────────────────────────────────────────

/** The projects a draft can start in, the one an agent last started in first. */
function projectRows(ctx: PaletteContext, run: (repoPath: string) => void): PaletteItem[] {
  const { s } = ctx;
  const projects = [...(s.workspace?.projects ?? [])].sort((a, b) =>
    a.path === s.lastRepoPath ? -1 : b.path === s.lastRepoPath ? 1 : 0,
  );
  return projects.map((p) => ({
    id: `pick:${p.path}`,
    label: p.name,
    hint: p.path,
    meta: p.path === s.lastRepoPath ? "Last used" : undefined,
    leading: { kind: "icon", name: "folder" },
    section: SEC.projects,
    keywords: basename(p.path),
    run: () => run(p.path),
  }));
}

/** The rows of one drill-in page. */
export function pageRows(ctx: PaletteContext, page: PageId): PaletteItem[] {
  const { s } = ctx;
  switch (page) {
    case "root":
      return rootItems(ctx);
    case "new-agent":
      return projectRows(ctx, (repo) => void s.createDraft(repo));
    case "workflow":
      return projectRows(ctx, (repo) => void s.createDraft(repo, { kind: "workflow" }));
    case "environment": {
      const order = (a: EnvironmentEntry, b: EnvironmentEntry) =>
        a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "local" ? -1 : 1;
      return Object.values(s.environments)
        .sort(order)
        .map((e) => environmentRow(s, e, SEC.environments));
    }
    case "settings":
      return entityItems(ctx)
        .filter((it) => it.section === SEC.settings)
        .map((it) => ({ ...it, label: it.label.replace(/^Settings › /, "") }));
    case "editor": {
      const agent = surfaceAgent(s);
      if (!agent) return [];
      const preferred = localStorage.getItem(EDITOR_PREF_KEY);
      return editorsFor(agent, ctx.editors).map((e) => ({
        id: `editor:${e.id}`,
        label: e.label,
        meta: e.id === preferred ? "Preferred" : undefined,
        leading: { kind: "icon", name: e.kind === "terminal" ? "terminal" : "code" },
        section: "Editors",
        run: () => {
          localStorage.setItem(EDITOR_PREF_KEY, e.id);
          openEditor(agent, e.id).catch((err) => s.setLastError(String(err)));
        },
      }));
    }
    case "accent":
      return ACCENTS.map((a) => ({
        id: `accent:${a.id}`,
        label: a.label,
        meta: a.id === s.accent ? "Current" : undefined,
        leading: { kind: "swatch", color: a.color },
        section: "Accent",
        run: () => s.setAccent(a.id),
      }));
  }
}

/** Every row the root page can show, before any query is applied. */
export function rootItems(ctx: PaletteContext): PaletteItem[] {
  return [...shortcutItems(ctx), ...agentItems(ctx), ...pageItems(ctx), ...entityItems(ctx)];
}

// ── Grouping ──────────────────────────────────────────────────────────────────

/** What the root page shows before anything is typed: a few actions that fit
 *  the surface in front, what was run lately, and the agents worth a look.
 *  The surface decides the first group — stopping is only worth offering
 *  while something runs — and the other two sections fill in behind it. */
export function emptySections(
  ctx: PaletteContext,
  items: PaletteItem[],
  recents: readonly string[],
): Section[] {
  const byId = new Map(items.map((it) => [it.id, it]));
  const suggestedIds = suggestedFor(ctx);
  const suggested = suggestedIds
    .map((id) => byId.get(id))
    .filter((it): it is PaletteItem => it !== undefined)
    .slice(0, SUGGESTED_CAP);
  const taken = new Set(suggested.map((it) => it.id));
  const recent = recents
    .map((id) => byId.get(id))
    .filter((it): it is PaletteItem => it !== undefined && !taken.has(it.id))
    .slice(0, RECENT_CAP);
  const selected = ctx.s.selectedAgentId;
  const jump = items
    .filter((it) => it.section === SEC.agents && it.id !== `agent:${selected}`)
    .slice(0, JUMP_CAP);
  const plain = (items: PaletteItem[]): Row[] => items.map((item) => ({ item, indices: [] }));
  return [
    { label: SEC.suggested, rows: plain(suggested) },
    { label: SEC.recent, rows: plain(recent) },
    { label: SEC.jump, rows: plain(jump) },
  ].filter((sec) => sec.rows.length > 0);
}

/** The ids worth surfacing first on each surface, best first. Missing ids
 *  (an action that doesn't apply) are skipped, so the list reads as what can
 *  be done, not what might. */
function suggestedFor(ctx: PaletteContext): string[] {
  const hasArchived = (ctx.s.workspace?.agents ?? []).some((a) => a.archive);
  switch (activeSurface(ctx.s).kind) {
    case "agent":
      return [
        "stopAgent",
        "archiveAgent",
        "commit-pr",
        "view-pr",
        "openInEditor",
        "panelGit",
        "newAgent",
      ];
    case "draft":
      return ["focusComposer", "new-agent-in", "workflow-in", "addProject", "home"];
    case "run":
      return ["home", "newAgent", "togglePanel", "history"];
    case "project":
      return ["projectSettings", "newAgent", "workflow-in", "home"];
    case "home":
      return ["newAgent", "addProject", "workflow-in", hasArchived ? "history" : "usage", "usage"];
    default:
      return ["home", "newAgent", "shortcuts"];
  }
}

/** The rows that answer `query`, grouped by section, best section first and
 *  best row first within it. While there is a query each section is capped —
 *  a longer query is how the rest is reached; with none (a page just opened)
 *  every row shows in its own order. */
export function searchSections(items: PaletteItem[], query: string): Section[] {
  const searching = query.trim().length > 0;
  const scored = new Map<string, (Row & { score: number })[]>();
  for (const item of items) {
    const m = bestMatch(query, item.label, [item.keywords, item.hint].filter(Boolean).join("  "));
    if (!m) continue;
    const list = scored.get(item.section) ?? [];
    list.push({ item, score: m.score, indices: m.indices });
    scored.set(item.section, list);
  }
  return [...scored.entries()]
    .map(([label, rows]) => {
      if (searching) rows.sort((a, b) => b.score - a.score);
      const cap = searching ? (SECTION_CAP[label] ?? DEFAULT_CAP) : rows.length;
      return {
        label,
        best: rows[0].score,
        rows: rows.slice(0, cap).map(({ item, indices }) => ({ item, indices })),
      };
    })
    .sort((a, b) => b.best - a.best)
    .map(({ label, rows }) => ({ label, rows }));
}
