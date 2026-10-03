// The Git tab's status strip, as data. Mirrors the desktop's `describeHeader`
// (src/components/RightPanel/GitPanel/StatusHeader.tsx): the same panel state
// from `deriveState`, the same merge-gate vocabulary, the same pills — so the
// phone and the Mac say the same thing about a checkout.

import type { GitState } from "@desktop/api/types/git";
import type { PrChecks, PrState } from "@desktop/api/types/pr";
import { deriveState } from "@desktop/components/RightPanel/primaryActions";
import { describeMergeGate, type MergeGateTone, mergeGateLabel } from "@desktop/mergeGate";

/** Tint of the strip: clean=green · uncommitted=amber · pushed/PR=accent ·
 *  fixable (conflicts, failing checks)=orange · ready=green · merged=purple. */
export type GitHeaderKind = "clean" | "changes" | "info" | "att" | "ready" | "merged" | "neutral";

export interface GitHeader {
  kind: GitHeaderKind;
  pill?: string;
  /** Primary mono text: the branch, or a PR phrase like "ready to merge". */
  text: string;
  /** Trailing muted text after `text` (e.g. "← main"). */
  sub?: string;
  /** A leading status dot instead of a pill (clean state). */
  dot?: boolean;
  /** Show the +adds/−dels summary on the right (changes state). */
  diff?: boolean;
  /** There is a PR worth linking to from the strip: the one the pill names, or
   *  the open/merged PR that new work on this branch updates or follows. */
  prLink?: boolean;
}

const KIND_BY_TONE: Record<MergeGateTone, GitHeaderKind> = {
  ready: "ready",
  warn: "changes",
  attention: "att",
  info: "info",
};

/** A checkout Fletch will not run git in (`GitState.blocked_config`). Its git
 *  state is the last one read before the block, so nothing from it is shown. */
const BLOCKED: GitHeader = { kind: "att", pill: "Git paused", text: "blocking settings" };

export function describeGitHeader(
  git: GitState | null | undefined,
  pr: PrState | null | undefined,
  checks: PrChecks | null | undefined,
  branch: string,
  base: string,
): GitHeader {
  if (git?.blocked_config?.length) return BLOCKED;
  const state = deriveState(git ?? null, pr ?? null);
  const n = pr?.number;
  const prLink = pr?.state === "open" || pr?.state === "merged";
  switch (state) {
    case "loading":
      return { kind: "neutral", text: "Loading…" };
    case "changes":
      return { kind: "changes", pill: "Uncommitted", text: branch, diff: true, prLink };
    case "pushed":
      return { kind: "info", pill: "Pushed", text: branch, prLink };
    case "conflicts":
      return { kind: "att", pill: "Conflicts", text: branch, sub: `← ${base}` };
    case "pr-open": {
      const gate = describeMergeGate(checks?.merge_state ?? null, {
        checksFailed: checks?.failed ?? 0,
        mergeable: pr?.mergeable ?? "unknown",
      });
      return {
        kind: KIND_BY_TONE[gate.tone],
        pill: n != null ? `PR #${n}` : "PR",
        text: mergeGateLabel(gate.situation, base),
        prLink: true,
      };
    }
    case "pr-closed":
      return {
        kind: "neutral",
        pill: n != null ? `Closed #${n}` : "Closed",
        text: branch,
        prLink: true,
      };
    case "merged":
      return {
        kind: "merged",
        pill: n != null ? `Merged #${n}` : "Merged",
        text: `→ ${base}`,
        prLink: true,
      };
    default:
      return { kind: "clean", text: branch, sub: `← ${base}`, dot: true };
  }
}
