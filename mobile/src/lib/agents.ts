// Derivations over the workspace snapshot: which agents are active, what a
// row's status word is, the primary checkout, and colour for a project chip.

import type { AgentRecord, AgentStatus, ProjectRef, Workspace } from "@desktop/api/types/agent";
import { PROVIDERS } from "@desktop/data/providers";

export const STATUS_LABEL: Record<AgentStatus, string> = {
  spawning: "Starting",
  running: "Working",
  idle: "Idle",
  stopped: "Stopped",
  error: "Error",
};

export const EFFORTS = ["low", "medium", "high", "max"];

/** Statuses that put an agent in a project's "Active" filter. */
export const isActive = (a: AgentRecord) =>
  a.status === "running" || a.status === "spawning" || a.status === "error";

export const isBusy = (a: AgentRecord) => a.status === "running" || a.status === "spawning";

/** `repos[0]` is the workspace repo the agent was spawned in. */
export const primaryRepo = (a: AgentRecord) => a.repos[0];

export const branchOf = (a: AgentRecord) => primaryRepo(a)?.branch ?? "—";

export const baseOf = (a: AgentRecord) => primaryRepo(a)?.parent_branch ?? "main";

/** A project's live agents, newest first. The host hands the snapshot back in
 *  `created_at` order, so the list has to be reversed here or the most recent
 *  agent lands at the bottom — same ordering the desktop sidebar applies. */
export const agentsOfProject = (ws: Workspace | null, projectId: string) =>
  (ws?.agents ?? [])
    .filter((a) => a.project_id === projectId && !a.archive)
    .sort((a, b) => (a.created_at < b.created_at ? 1 : -1));

export const providerLabel = (id: string) => PROVIDERS.find((p) => p.id === id)?.label ?? id;

export const providerShort = (id: string) =>
  PROVIDERS.find((p) => p.id === id)?.short ?? id.slice(0, 2).toUpperCase();

export const providerHue = (id: string) => PROVIDERS.find((p) => p.id === id)?.hue ?? 0;

/** A stable hue per project so its swatch keeps its colour across launches. */
export function projectHue(project: ProjectRef): number {
  let h = 0;
  for (const ch of project.project_id) h = (h * 31 + ch.charCodeAt(0)) % 360;
  return h;
}

export const hueColor = (hue: number) => `oklch(0.72 0.12 ${hue})`;

/** GitHub `owner/repo` when the remote says so, else the folder name. */
export function repoLabel(project: ProjectRef, remoteUrl?: string | null): string {
  const match = remoteUrl ? /github\.com[/:]([^/]+\/[^/.]+)/.exec(remoteUrl) : null;
  return match ? match[1] : (project.path.split("/").pop() ?? project.name);
}

/** Compact model name for chips: drop a provider prefix and prettify dashes. */
export function modelLabel(model: string | null | undefined): string {
  if (!model) return "default";
  return model.replace(/^(claude|anthropic|openai|google)[-/]/, "");
}
