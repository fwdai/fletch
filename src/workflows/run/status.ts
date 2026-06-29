// run/status.ts — display mapping for run / step statuses (shared by the monitor
// and the runs list).

import type { IconName } from "../../components/Icon";
import type { RunStatus, RunStepStatus } from "./types";

const GREEN = "oklch(0.72 0.15 150)";
const AMBER = "oklch(0.78 0.14 75)";

export function stepChip(s: RunStepStatus): { label: string; icon: IconName; tone: string } {
  switch (s) {
    case "running":
      return { label: "running", icon: "dot", tone: "var(--accent)" };
    case "done":
      return { label: "done", icon: "check", tone: GREEN };
    case "error":
      return { label: "error", icon: "close", tone: "var(--danger)" };
    case "blocked":
      return { label: "blocked", icon: "minus", tone: AMBER };
    case "awaiting_approval":
      return { label: "needs approval", icon: "user", tone: AMBER };
    case "pending":
    default:
      return { label: "pending", icon: "dot", tone: "var(--fg-3)" };
  }
}

export function runChip(s: RunStatus): { label: string; tone: string } {
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
    case "pending":
    default:
      return { label: "pending", tone: "var(--fg-3)" };
  }
}
