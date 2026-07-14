// run/status.ts — display mapping for the workflows-v1 run / attempt / paused
// vocabulary (spec §6.2). Shared by the monitor, the attempt rail, the sidebar
// row badge, and the paused-reason banner. Pure presentation; the semantics
// live in the Rust engine.

import type { WfAttemptStatus, WfPausedReason, WfRunStatus } from "../../api";
import type { IconName } from "../../components/Icon";

export const GREEN = "oklch(0.72 0.15 150)";
export const AMBER = "oklch(0.78 0.14 75)";

export interface Chip {
  label: string;
  icon: IconName;
  tone: string;
}

/** A step attempt's status (spec §6.2). `abandoned` is the dimmed rail state. */
export function attemptChip(s: WfAttemptStatus): Chip {
  switch (s) {
    case "spawning":
      return { label: "spawning", icon: "dot", tone: "var(--fg-3)" };
    case "running":
      return { label: "running", icon: "dot", tone: "var(--accent)" };
    case "gating":
      return { label: "checking gate", icon: "dot", tone: "var(--accent)" };
    case "done":
      return { label: "done", icon: "check", tone: GREEN };
    case "error":
      return { label: "error", icon: "close", tone: "var(--danger)" };
    case "blocked":
      return { label: "blocked", icon: "minus", tone: AMBER };
    case "awaiting_approval":
      return { label: "needs approval", icon: "user", tone: AMBER };
    case "abandoned":
      return { label: "abandoned", icon: "minus", tone: "var(--fg-3)" };
    default:
      return { label: "pending", icon: "dot", tone: "var(--fg-3)" };
  }
}

export function runChip(s: WfRunStatus): { label: string; tone: string } {
  switch (s) {
    case "running":
      return { label: "running", tone: "var(--accent)" };
    case "done":
      return { label: "complete", tone: GREEN };
    case "failed":
      return { label: "failed", tone: "var(--danger)" };
    case "paused":
      return { label: "paused", tone: AMBER };
    case "canceled":
      return { label: "canceled", tone: "var(--fg-3)" };
    default:
      return { label: "pending", tone: "var(--fg-3)" };
  }
}

/** One-line plain-language name for why a run paused (spec §6.2), used by the
 *  sidebar badge and as the banner title. */
export function pausedLabel(reason: WfPausedReason): string {
  switch (reason) {
    case "approval":
      return "needs approval";
    case "question":
      return "awaiting answer";
    case "blocked_gate":
      return "gate not met";
    case "budget_exceeded":
      return "budget reached";
    case "conflict":
      return "merge conflict";
    case "stalled":
      return "stalled";
  }
}
