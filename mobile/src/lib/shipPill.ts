import type { PrChecks, PrState } from "@desktop/api/types/pr";
import { describeMergeGate, mergeGateLabel } from "@desktop/mergeGate";

/** The `.pill` tone classes the agent row knows; "" is the untinted default. */
export type PillTone = "ok" | "warn" | "merged" | "";

export interface Pill {
  text: string;
  tone: PillTone;
}

/** The agent row's PR pill, off the same merge-gate classification the Ship
 *  tab's strip and the desktop sidebar render from — so a row and the screen
 *  behind it never disagree about whether a PR is ready. `checks` is the last
 *  known rollup (null before the first sweep says anything), in which case the
 *  gate falls back to GitHub's coarse `mergeable` verdict. */
export function prPill(
  pr: PrState | null | undefined,
  checks: PrChecks | null | undefined,
): Pill | null {
  if (!pr) return null;
  if (pr.state === "merged") return { text: "merged", tone: "merged" };
  if (pr.state === "closed") return { text: `#${pr.number} closed`, tone: "" };
  const gate = describeMergeGate(checks?.merge_state ?? null, {
    checksFailed: checks?.required_failing.length ?? 0,
    mergeable: pr.mergeable,
  });
  const tone: PillTone = gate.tone === "ready" ? "ok" : gate.tone === "attention" ? "warn" : "";
  return { text: `#${pr.number} · ${mergeGateLabel(gate.situation)}`, tone };
}
