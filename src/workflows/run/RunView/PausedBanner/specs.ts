// The paused/failed banner state-machine mapper (spec §14.2): each state a run
// can rest in maps to its plain-language cause and the action(s) that move it
// forward. No dead buttons — every paused reason wires to its command.

import { api, type WfRun } from "../../../../api";
import type { IconName } from "../../../../components/Icon";
import { pausedLabel } from "../../status";

export interface Action {
  label: string;
  icon: IconName;
  run: (runId: string) => Promise<void>;
  primary?: boolean;
}

export interface BannerSpec {
  tone: "amber" | "danger";
  title: string;
  body: string;
  actions: Action[];
}

const RETRY: Action = { label: "Retry", icon: "refresh", run: api.wfRetry, primary: true };
const RESUME: Action = { label: "Resume", icon: "play", run: api.wfResume, primary: true };
// Conflict resolution (§12.3): let an agent resolve the pinned conflict snapshot,
// or continue after the user has resolved it in the run repo's integration
// worktree and committed.
const RESOLVE_AGENT: Action = {
  label: "Resolve with agent",
  icon: "play",
  run: (id) => api.wfResolveConflict(id, "agent"),
  primary: true,
};
const RESOLVE_HUMAN: Action = {
  label: "I resolved it — continue",
  icon: "check",
  run: (id) => api.wfResolveConflict(id, "human"),
};

export function specFor(run: WfRun, detail?: string): BannerSpec | null {
  if (run.status === "failed") {
    return {
      tone: "danger",
      title: "Run failed",
      body: run.error || "The run stopped with an unrecoverable error.",
      actions: [],
    };
  }
  if (run.status !== "paused" || !run.paused_reason) return null;

  const title = `Paused — ${pausedLabel(run.paused_reason)}`;
  switch (run.paused_reason) {
    case "approval":
      // The action is the review surface (rendered inline below), not a bare
      // Approve button — a merge decision deserves the evidence first.
      return {
        tone: "amber",
        title,
        body: detail || "A step is waiting for your review before handing off to the next.",
        actions: [],
      };
    case "blocked_gate":
      return {
        tone: "amber",
        title,
        body: detail || "A step finished but its gate isn't satisfied yet.",
        actions: [RETRY],
      };
    case "stalled":
      return {
        tone: "amber",
        title,
        body: detail || "A step stopped making progress and didn't recover after a nudge.",
        actions: [RETRY, RESUME],
      };
    case "budget_exceeded":
      return {
        tone: "amber",
        title,
        body: detail || "The run reached a configured budget. Raise it to keep going.",
        // The resume form (rendered in the body) raises the caps and re-drives;
        // a plain Resume would just re-hit the same budget.
        actions: [],
      };
    case "question":
      return {
        tone: "amber",
        title,
        body: detail || "A step asked a question and is waiting for your answer.",
        actions: [],
      };
    case "conflict":
      return {
        tone: "amber",
        title,
        body:
          detail ||
          "Merging parallel work hit a conflict. Let an agent resolve it, or resolve it " +
            "yourself in the run's integration worktree and continue.",
        actions: [RESOLVE_AGENT, RESOLVE_HUMAN],
      };
  }
}
