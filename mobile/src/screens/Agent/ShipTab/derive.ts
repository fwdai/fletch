// The Ship tab, as data: the desktop Git panel's own models, read for the
// phone. The ladder rows (`buildLadder`), the footer (`buildFooter`) and the
// one action gate (`gateAction`) are the desktop's, so the two clients never
// say different things about the same checkout. What is the phone's: the strip
// it shows on top, which footer keys it can run (`actions.ts`), and the PR
// sheet as the manual alternative to a commit playbook.

import type { GitState } from "@desktop/api/types/git";
import type { MergeState, PrChecks, PrComments, PrState } from "@desktop/api/types/pr";
import { gateAction } from "@desktop/components/RightPanel/GitPanel/actionGate";
import {
  applyGate,
  buildFooter,
  endingActions,
  type FooterAction,
  type FooterGroup,
} from "@desktop/components/RightPanel/GitPanel/footerModel";
import {
  buildLadder,
  type LadderRow,
  type RungMark,
  type RungTone,
} from "@desktop/components/RightPanel/GitPanel/ladderModel";
import { type DelegationKind, delegationLabel } from "@desktop/delegation";
import { describeMergeGate } from "@desktop/mergeGate";
import { hasWorkSinceMerge } from "@desktop/readiness";
import { COMMIT_KEYS, MANUAL_KEY, type ShipFacts, shipOp, shipRefusal } from "./actions";

/** Tint of the strip: clean/ready=green · changes=amber · info=accent ·
 *  att=orange · merged=purple · neutral=grey · working=accent with a spinner. */
export type StripKind =
  | "clean"
  | "changes"
  | "info"
  | "att"
  | "ready"
  | "merged"
  | "neutral"
  | "working";

export interface ShipStrip {
  kind: StripKind;
  text: string;
  /** Trailing muted text after `text` (e.g. "← main"). */
  sub?: string;
  /** Show the +adds/−dels summary on the right. */
  diff?: boolean;
  /** There is a PR worth linking to from the strip. */
  prLink?: boolean;
}

export interface ShipFooterView {
  /** The one button, or null when the ladder's next step is one the phone
   *  can't run (pull, publish, …) or nothing is known yet. */
  primary: FooterAction | null;
  /** Everything else the "More" sheet offers, gated, with the desktop's group
   *  titles. Never holds `primary`. */
  groups: FooterGroup[];
  /** Why the primary is disabled, when that is worth a line (`applyGate`). */
  mainReason: string | null;
}

export interface ShipView {
  ladder: LadderRow[];
  strip: ShipStrip;
  footer: ShipFooterView;
}

export interface ShipInput {
  git: GitState | null;
  pr: PrState | null;
  /** `undefined` until the first checks read lands — an open PR's merge gate
   *  reads as "unknown" meanwhile, as on the desktop (`useGitPanelData`). */
  checks: PrChecks | null | undefined;
  comments: PrComments | null;
  base: string;
  /** A playbook the host has in flight for this agent (`delegation:changed`). */
  delegation: DelegationKind | null;
  /** The host answers `merge_pr` (`hostSupports`). */
  canMerge: boolean;
}

/** The strip's tint for a ladder row: its attention colour when it has one,
 *  else its mark. */
const STRIP_OF_TONE: Record<RungTone, StripKind> = {
  warn: "changes",
  att: "att",
  danger: "att",
  info: "info",
  merged: "merged",
  neutral: "neutral",
};
const STRIP_OF_MARK: Record<RungMark, StripKind> = {
  done: "ready",
  running: "info",
  attention: "att",
  idle: "neutral",
};

export const stripKind = (row: Pick<LadderRow, "mark" | "tone">): StripKind =>
  row.tone ? STRIP_OF_TONE[row.tone] : STRIP_OF_MARK[row.mark];

/** A checkout Fletch will not run git in (`GitState.blocked_config`). Its git
 *  state is the last one read before the block, so nothing from it is shown. */
const BLOCKED: ShipStrip = { kind: "att", text: "Git paused", sub: "blocking settings" };
const NO_FOOTER: ShipFooterView = { primary: null, groups: [], mainReason: null };

const MANUAL: FooterAction = { key: MANUAL_KEY, label: "Write the message yourself", icon: "edit" };

export function shipView(input: ShipInput): ShipView {
  const { git, pr, comments, base, delegation, canMerge } = input;
  const checks = input.checks ?? null;
  const prOpen = pr?.state === "open";
  const mergeState: MergeState | null =
    checks?.merge_state ?? (prOpen && input.checks === undefined ? "unknown" : null);
  const prLink = prOpen || pr?.state === "merged";

  if (git?.blocked_config?.length) return { ladder: [], strip: BLOCKED, footer: NO_FOOTER };

  const facts: ShipFacts = { base, pr, checks, comments };
  const runnable = (a: FooterAction) => shipOp(a.key, facts) != null;
  const opensSheet = (a: FooterAction) => shipOp(a.key, facts)?.kind === "manual";

  // The gate `useFooterModel` builds, over the phone's own host table. The
  // phone's in-flight merge is held by `useShipActions`, not here.
  const { mergeAllowed } = describeMergeGate(checks ? mergeState : null, {
    checksFailed: checks?.required_failing.length ?? 0,
    mergeable: pr?.mergeable ?? "unknown",
  });
  const gate = (key: string) =>
    gateAction(key, {
      hostRefusal: (k) => shipRefusal(k, facts, canMerge),
      laneBranch: pr?.branch ?? null,
      checkoutBranch: git?.branch || null,
      mergeAllowed,
      delegationActive: delegation != null,
      busy: false,
      primaryCheckout: true,
      workSinceMerge: git != null && hasWorkSinceMerge(git),
    });

  const ladder = buildLadder({ git, pr, checks, mergeState, comments, base });
  const model = buildFooter({
    git,
    pr,
    checks,
    mergeState,
    comments,
    base,
    // The phone always commits straight to a PR; it has no mode to pick, so
    // the radio group below is dropped.
    commitMode: "agent-commit-pr",
    customActive: false,
    // The phone has no read of the host's GitHub connection. It assumes one,
    // and a host without it refuses the push or PR with a reason.
    githubConnected: true,
    hasOrigin: git?.has_origin ?? true,
  });
  const committing = COMMIT_KEYS.has(model.primary.key);
  const groups: FooterGroup[] = [
    // Every row the phone runs as the PR sheet folds into the one manual row.
    ...model.groups
      .filter((g) => !g.radio)
      .map((g) => ({ ...g, items: g.items.filter((i) => !opensSheet(i)) })),
    // The merged PR's ending actions sit on its row on the desktop; the phone
    // has no row to put them on, so they are a group of their own.
    ...(pr?.state === "merged" ? [{ title: "Workspace", items: endingActions() }] : []),
    ...(committing ? [{ items: [MANUAL] }] : []),
  ];
  const gated = applyGate({ primary: model.primary, groups }, gate);
  const primary = runnable(gated.primary) ? gated.primary : null;
  const footer: ShipFooterView = {
    primary,
    groups: gated.groups
      .map((g) => ({ ...g, items: g.items.filter(runnable) }))
      .filter((g) => g.items.length > 0),
    mainReason: primary ? gated.mainReason : null,
  };

  // The strip: the playbook in flight, else the row the ladder's next step is
  // about — or, with nothing to do, the PR's row when there is one.
  let strip: ShipStrip;
  if (delegation) {
    strip = { kind: "working", text: delegationLabel(delegation), prLink };
  } else if (!git) {
    strip = { kind: "neutral", text: "Loading…" };
  } else {
    const at = ladder.find((r) => r.current) ?? (pr ? ladder.find((r) => r.key === "pr") : null);
    strip = at
      ? { kind: stripKind(at), text: at.summary, diff: at.key === "changes", prLink }
      : { kind: "clean", text: "Nothing to ship yet", sub: `← ${base}` };
  }
  return { ladder, strip, footer };
}
