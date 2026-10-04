// Derivations over the workspace snapshot: which agents are active, what a
// row's status word is, the primary checkout, and colour for a provider chip.
// Project derivations live in lib/projects.

import type { AgentRecord, AgentStatus, Workspace } from "@desktop/api/types/agent";
import type { GitState } from "@desktop/api/types/git";
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

/** The one busy signal every surface renders from, as on the desktop: the
 *  host's status — `running` while a turn is in flight, whoever started it,
 *  `spawning` while the process it needs comes up — plus this device's own
 *  send until the host answers it with a status (`sending`), so the UI reads
 *  "working" from the tap rather than from the round trip. */
export const isAgentBusy = (s: { sending: Record<string, boolean> }, a: AgentRecord) =>
  isBusy(a) || !!s.sending[a.id];

/** `repos[0]` is the workspace repo the agent was spawned in. */
export const primaryRepo = (a: AgentRecord) => a.repos[0];

export const branchOf = (a: AgentRecord) => primaryRepo(a)?.branch ?? "—";

/** Label for the ref a checkout is actually sitting on, from its live git
 *  state: the branch name, or the short SHA when HEAD is detached — which is
 *  how an agent workspace starts out. Prefer this over `branchOf` wherever the
 *  git state is at hand: the record's `branch` column stays null until a push
 *  names a branch, so it renders an em dash for most of an agent's life. */
export const refLabel = (git: GitState | undefined | null) =>
  git?.branch || git?.head_sha?.slice(0, 7) || "—";

export const baseOf = (a: AgentRecord) => primaryRepo(a)?.parent_branch ?? "main";

/** A project's live agents, newest first. The host hands the snapshot back in
 *  `created_at` order, so the list has to be reversed here or the most recent
 *  agent lands at the bottom — same ordering the desktop sidebar applies.
 *  Agents spawned in the same millisecond (a workflow's fan-out) break the tie
 *  on id, so the order is total rather than left to the sort's discretion —
 *  the tie-break `deriveStepChildren` already uses. */
export const agentsOfProject = (ws: Workspace | null, projectId: string) =>
  (ws?.agents ?? [])
    .filter((a) => a.project_id === projectId && !a.archive)
    .sort((a, b) => b.created_at.localeCompare(a.created_at) || a.id.localeCompare(b.id));

export const providerLabel = (id: string) => PROVIDERS.find((p) => p.id === id)?.label ?? id;

export const providerShort = (id: string) =>
  PROVIDERS.find((p) => p.id === id)?.short ?? id.slice(0, 2).toUpperCase();

export const providerHue = (id: string) => PROVIDERS.find((p) => p.id === id)?.hue ?? 0;

export const hueColor = (hue: number) => `oklch(0.72 0.12 ${hue})`;

/** Compact model name for chips: drop a provider prefix and prettify dashes. */
export function modelLabel(model: string | null | undefined): string {
  if (!model) return "default";
  return model.replace(/^(claude|anthropic|openai|google)[-/]/, "");
}
