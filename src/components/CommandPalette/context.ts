// What every row builder reads and the few derivations they share. Pure:
// a store snapshot plus what lives outside it, and nothing that renders.

import type { AgentRecord, DetectedEditor, WfRun } from "@/api";
import { agentDotStatus } from "@/components/TitleBar/WorkspaceStatus/derive";
import type { AppState } from "@/store/types";
import { basename, firstLine } from "@/util/format";
import { closeScreens } from "@/util/shortcuts";

/** What the rows are built from. `s` is a snapshot of the store; the rest is
 *  what the store does not hold. */
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

/** Section labels: the command groups first, then the things to jump to,
 *  then the empty state's own three. */
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
  editors: "Editors",
  accent: "Accent",
  suggested: "Suggested",
  recent: "Recent",
  jump: "Jump to",
} as const;

export const projectName = (s: AppState, repoPath: string | undefined): string =>
  (repoPath && s.workspace?.projects.find((p) => p.path === repoPath)?.name) ||
  (repoPath ? basename(repoPath) : "");

/** The agent's row title, as the sidebar writes it: its own title, else the
 *  first line of the task, else the codename. */
export const agentTitle = (a: AgentRecord): string => a.title || firstLine(a.task) || a.name;

/** The agents the sidebar lists: live, the user's own, not a run's step. */
export function sidebarAgents(s: AppState): AgentRecord[] {
  return (s.workspace?.agents ?? []).filter((a) => !a.archive && !a.purpose && !a.owner_run_id);
}

export const isWorking = (a: AgentRecord) => a.status === "running" || a.status === "spawning";
export const isArchivable = (a: AgentRecord) =>
  a.status === "idle" || a.status === "stopped" || a.status === "error";

/** Agents in the order someone would want them: a question waiting first,
 *  then whatever is working, then the rest newest first. */
export function byAttention(s: AppState, agents: AgentRecord[]): AgentRecord[] {
  const rank = (a: AgentRecord) => {
    const status = agentDotStatus(a.status, s.pendingToolUse[a.id]);
    return status === "waiting" ? 0 : status === "running" ? 1 : status === "error" ? 2 : 3;
  };
  return [...agents].sort((a, b) => rank(a) - rank(b) || (a.created_at < b.created_at ? 1 : -1));
}

/** Run a row that takes the user somewhere: every takeover (Settings, a
 *  project page, Usage, History) closes first, so what the row selects or
 *  opens is in front rather than behind. A selection on its own does not
 *  leave those surfaces, which is how a draft started from Settings used to
 *  land invisibly. */
export function go(s: AppState, fn: () => void): void {
  closeScreens(s);
  fn();
}

/** Start a draft where the user can see it. */
export function startDraft(s: AppState, repoPath: string, kind: "agent" | "workflow"): void {
  go(s, () => {
    if (kind === "workflow") void s.createDraft(repoPath, { kind });
    else void s.createDraft(repoPath);
  });
}
