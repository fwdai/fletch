// What every row builder reads and the few derivations they share. Pure:
// a store snapshot plus what lives outside it, and nothing that renders.

import type { AgentRecord, Api, DetectedEditor, WfRun } from "@/api";
import { agentDotStatus } from "@/components/TitleBar/WorkspaceStatus/derive";
import { type EnvironmentId, LOCAL_ENVIRONMENT_ID } from "@/store/environments";
import type { AppState } from "@/store/types";
import { basename, firstLine } from "@/util/format";
import { closeScreens } from "@/util/shortcuts";

/** One host, as the rows that cover every host see it. */
export interface PaletteHost {
  id: EnvironmentId;
  /** A paired host's name, for the rows that say where they run; null for
   *  This Mac. */
  name: string | null;
  /** A snapshot of the store as this host sees it, with its own actions. */
  s: AppState;
}

/** What the rows are built from. `s` is a snapshot of the store as the host on
 *  screen sees it — the palette acts on what is in front — and `hosts` every
 *  host's, This Mac's first, for the rows that cover them all; the rest is
 *  what the store does not hold. */
export interface PaletteContext {
  s: AppState;
  hosts: readonly PaletteHost[];
  /** Put a host on screen, before a row selects something there. */
  show: (hostId: EnvironmentId) => void;
  /** The engine of the host on screen. */
  api: Api;
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
  editors: "Editors",
  accent: "Accent",
  suggested: "Suggested",
  recent: "Recent",
  jump: "Jump to",
} as const;

export const projectName = (s: AppState, repoPath: string | undefined): string =>
  (repoPath && s.workspace?.projects.find((p) => p.path === repoPath)?.name) ||
  (repoPath ? basename(repoPath) : "");

/** The id of an agent's row. This Mac's agents keep the bare id the recents
 *  list has always held; a paired host's carry the host, as ids repeat. */
export const agentItemId = (hostId: EnvironmentId, agentId: string): string =>
  hostId === LOCAL_ENVIRONMENT_ID ? `agent:${agentId}` : `agent:${hostId}:${agentId}`;

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

/** Every host's agents, each with its host, in the order someone would want
 *  them whichever host they run on: a question waiting first, then whatever
 *  is working, then the rest newest first. */
export function byAttention(
  hosts: readonly PaletteHost[],
): { host: PaletteHost; agent: AgentRecord }[] {
  const rank = ({ host, agent: a }: { host: PaletteHost; agent: AgentRecord }) => {
    const status = agentDotStatus(a.status, host.s.pendingToolUse[a.id]);
    return status === "waiting" ? 0 : status === "running" ? 1 : status === "error" ? 2 : 3;
  };
  return hosts
    .flatMap((host) => sidebarAgents(host.s).map((agent) => ({ host, agent })))
    .sort((a, b) => rank(a) - rank(b) || (a.agent.created_at < b.agent.created_at ? 1 : -1));
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
