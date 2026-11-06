// The command rows. Two layers, deliberately. The global shortcuts
// (`util/keymap.ts`) are rows here verbatim — same label, same chord, same
// action — so the palette is also where every shortcut is discovered. On
// top sit the commands a chord could never carry: the current agent's PR
// and branch, and the drill-in pages.

import type { IconName } from "@/components/Icon";
import { editorsFor, preferredEditor } from "@/components/TitleBar/OpenInEditor/editors";
import { ACCENTS } from "@/data/providers";
import { activeEntry } from "@/store/capabilities";
import { ADD_PROJECT_GATES, anyGateReason, gateReason } from "@/store/gates";
import { activeSurface, surfaceAgent, surfaceRepoPath } from "@/store/surface";
import type { AppState } from "@/store/types";
import { SHORTCUT_BY_ID } from "@/util/keymap";
import { newAgentRepoPath, runShortcutAction } from "@/util/shortcuts";
import {
  go,
  isArchivable,
  isWorking,
  type PaletteContext,
  projectName,
  SEC,
  sidebarAgents,
  startDraft,
} from "./context";
import type { PaletteItem } from "./types";

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
  /** Command semantics where the chord toggles: a row always does the thing
   *  its label says, never the reverse of it. Defaults to the chord's action. */
  run?: (ctx: PaletteContext) => void;
}

const onAgent = (ctx: PaletteContext) => surfaceAgent(ctx.s) !== null;
const onAgentOrDraft = (ctx: PaletteContext) => {
  const kind = activeSurface(ctx.s).kind;
  return kind === "agent" || kind === "draft";
};
const hasRepos = (ctx: PaletteContext) => (ctx.s.workspace?.repos.length ?? 0) > 0;
const hasAgents = (ctx: PaletteContext) => sidebarAgents(ctx.s).length > 0;
const hasSurfacedProject = (ctx: PaletteContext) => surfaceRepoPath(ctx.s) !== undefined;
const forSurfacedProject = (ctx: PaletteContext) => {
  const repo = surfaceRepoPath(ctx.s);
  return repo ? `for ${projectName(ctx.s, repo)}` : undefined;
};

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
    disabled: (ctx) => anyGateReason(activeEntry(ctx.s), ADD_PROJECT_GATES),
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
      const current = a && preferredEditor(editorsFor(a, ctx.editors));
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
    disabled: (ctx) => gateReason(activeEntry(ctx.s), "runScripts"),
    keywords: "dev server  start  preview",
  },
  {
    id: "panelTerminal",
    icon: "terminal",
    section: SEC.agent,
    hint: "A shell in the checkout",
    when: (ctx) => onAgent(ctx) && ctx.s.features.terminal,
    disabled: (ctx) => gateReason(activeEntry(ctx.s), "sideShell"),
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
  // The project views. Their chords toggle the view; from the palette a row
  // only ever opens it, for the project in front.
  {
    id: "roadmap",
    icon: "map",
    section: SEC.goto,
    hint: forSurfacedProject,
    when: hasSurfacedProject,
    disabled: (ctx) => gateReason(activeEntry(ctx.s), "roadmap"),
    keywords: "board  plan  activity  shipped",
    run: (ctx) => {
      const repo = surfaceRepoPath(ctx.s);
      if (repo) go(ctx.s, () => ctx.s.openRoadmap(repo));
    },
  },
  {
    id: "projectSettings",
    icon: "settings",
    section: SEC.goto,
    hint: forSurfacedProject,
    when: hasSurfacedProject,
    keywords: "run command  setup  env  repo",
    run: (ctx) => {
      const repo = surfaceRepoPath(ctx.s);
      if (repo) go(ctx.s, () => ctx.s.openProjectSettings(repo));
    },
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

/** The global shortcuts that apply right now, as rows. */
export function shortcutItems(ctx: PaletteContext): PaletteItem[] {
  const items: PaletteItem[] = [];
  for (const row of SHORTCUT_ROWS) {
    const shortcut = SHORTCUT_BY_ID[row.id];
    if (!shortcut || (row.when && !row.when(ctx))) continue;
    const run = row.run;
    items.push({
      id: row.id,
      label: shortcut.label,
      hint: typeof row.hint === "function" ? row.hint(ctx) : row.hint,
      leading: { kind: "icon", name: typeof row.icon === "function" ? row.icon(ctx.s) : row.icon },
      section: row.section,
      shortcut: row.id,
      keywords: [row.keywords, shortcut.description].filter(Boolean).join("  "),
      disabled: row.disabled?.(ctx) ?? null,
      run: run ? () => run(ctx) : () => runShortcutAction(row.id),
      remember: true,
    });
  }
  return items;
}

/** The current agent's context actions: its PR and its branch. (Its project's
 *  roadmap is the `roadmap` shortcut row, which targets the same project.) */
export function agentItems(ctx: PaletteContext): PaletteItem[] {
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
        : gateReason(activeEntry(s), "delegateGit"),
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
  return items;
}

/** The drill-in pages and the app-level commands. */
export function pageItems(ctx: PaletteContext): PaletteItem[] {
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
    const one = projects.length === 1 ? projects[0].path : null;
    items.push({
      id: "workflow-in",
      label: one ? "Run a workflow" : "Run a workflow in…",
      hint: one ? `in ${projectName(s, one)}` : "A chain of agents on one branch",
      leading: { kind: "icon", name: "combine" },
      section: SEC.create,
      keywords: "pipeline  chain  automate  steps",
      disabled: gateReason(activeEntry(s), "workflows"),
      ...(one
        ? { run: () => startDraft(s, one, "workflow"), remember: true }
        : { page: "workflow" as const }),
    });
  }
  const repo = surfaceRepoPath(s);
  // Capture into the current project's inbox: the palette opens the board with
  // its capture field focused (typing the text here is a later step).
  if (repo && projects.find((p) => p.path === repo)?.project_id) {
    items.push({
      id: "roadmap-capture",
      label: "Add to roadmap…",
      hint: `Capture into ${projectName(s, repo)}'s inbox`,
      leading: { kind: "icon", name: "inbox" },
      section: SEC.create,
      keywords: "capture  idea  ticket  inbox  note  todo  backlog",
      disabled: gateReason(activeEntry(s), "roadmapCapture"),
      run: () => go(s, () => s.captureToRoadmap(repo)),
      remember: true,
    });
  }
  // The roadmap's Context tab for the current project, while the layer's
  // developer gate is open.
  if (repo && s.contextLayerEnabled) {
    items.push({
      id: "project-context",
      label: `Context · ${projectName(s, repo)}`,
      hint: "What the host knows about the project",
      leading: { kind: "icon", name: "graph" },
      section: SEC.goto,
      keywords: "entities  decisions  knowledge  review",
      disabled: gateReason(activeEntry(s), "roadmap"),
      run: () => go(s, () => s.openRoadmap(repo, "context")),
      remember: true,
    });
  }
  if (Object.keys(s.environments).length > 1) {
    items.push({
      id: "environment",
      label: "Switch environment…",
      hint: `Now on ${activeEntry(s).name}`,
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
