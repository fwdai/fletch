// run/status.ts — display mapping for the workflows-v1 run / attempt vocabulary
// (spec §6.2). Shared by the monitor, the attempt rail, and the sidebar row
// badge. Pure presentation; the semantics live in the Rust engine.

import type { WfAttemptStatus, WfRunStatus } from "../../api";
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
    case "canceled":
      return { label: "canceled", tone: "var(--fg-3)" };
    default:
      return { label: "pending", tone: "var(--fg-3)" };
  }
}
