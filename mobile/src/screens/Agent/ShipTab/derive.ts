// The Ship tab, as data: one strip of text, ONE primary button and the overflow
// under it, all read off the desktop's remediation ladder (`nextRung` in
// src/readiness.ts). The phone holds no second copy of that ladder — what to do
// next is the ladder's call; this module only decides how to say it and which
// host op or playbook a tap maps to.

import type { DelegationKind } from "@desktop/delegation";
import { delegationLabel } from "@desktop/delegation";
import { describeMergeGate, mergeGateLabel } from "@desktop/mergeGate";
import { type LadderContext, nextRung, type ReadinessInput, type Rung } from "@desktop/readiness";

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

export type ShipAction =
  /** Hand a playbook to the coding agent (`delegateGit`). */
  | {
      key: string;
      label: string;
      kind: "delegate";
      action: string;
      params?: Record<string, string>;
      delegation: DelegationKind;
    }
  /** The host's `merge_pr`. */
  | { key: "merge"; label: string; kind: "merge" }
  | { key: "archive"; label: string; kind: "archive" }
  | { key: "github"; label: string; kind: "github"; url: string }
  /** The manual commit / push / PR sheet (`openSheet("pr")`). */
  | { key: "manual"; label: string; kind: "manual" };

export interface ShipView {
  rung: Rung;
  strip: ShipStrip;
  /** The one button in the footer; null when there is nothing to press. */
  primary: ShipAction | null;
  /** Everything else reachable from the "More" sheet. Never holds `primary`. */
  more: ShipAction[];
}

export interface ShipExtra {
  /** A playbook the host has in flight for this agent, held or running
   *  (`delegation:changed` / `get_delegations`). */
  delegation: DelegationKind | null;
  /** The host answers `merge_pr` (`hostSupports`). */
  canMerge: boolean;
}

/** The playbooks that start with a commit or a push: the ones the Changes tab
 *  shows, and the ones with a manual alternative (the PR sheet). */
export const COMMIT_FAMILY: ReadonlySet<DelegationKind> = new Set<DelegationKind>([
  "commit",
  "commit-push",
  "commit-pr",
  "open-pr",
  "push",
]);

export const isCommitAction = (a: ShipAction | null): boolean =>
  a?.kind === "delegate" && COMMIT_FAMILY.has(a.delegation);

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
const capitalize = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** A checkout Fletch will not run git in (`GitState.blocked_config`). Its git
 *  state is the last one read before the block, so nothing from it is shown. */
const BLOCKED: ShipStrip = { kind: "att", text: "Git paused", sub: "blocking settings" };

export function describeShip(
  input: ReadinessInput,
  ctx: LadderContext,
  extra: ShipExtra,
): ShipView {
  const rung = nextRung(input, ctx);
  if (input.git?.blocked_config?.length) return { rung, strip: BLOCKED, primary: null, more: [] };

  const { pr, checks } = input;
  const open = pr?.state === "open" ? pr : null;
  const prLink = pr?.state === "open" || pr?.state === "merged";
  const gate = open
    ? describeMergeGate(checks?.merge_state ?? null, {
        checksFailed: checks?.required_failing.length ?? 0,
        mergeable: open.mergeable,
      })
    : null;
  const github: ShipAction | null = pr
    ? { key: "github", label: "Open on GitHub", kind: "github", url: pr.url }
    : null;

  let strip: ShipStrip;
  let primary: ShipAction | null;
  switch (rung.do) {
    case "wait":
      if (rung.why === "unknown-state") {
        strip = { kind: "neutral", text: "Loading…" };
        primary = null;
      } else {
        strip = { kind: "info", text: "GitHub is computing merge status", prLink };
        primary = github;
      }
      break;
    case "delegate": {
      const b = rung.blocker;
      const n = open?.number;
      switch (b.kind) {
        case "conflicted":
          strip = { kind: "att", text: `Conflicts in ${plural(b.paths.length, "file")}`, prLink };
          break;
        case "uncommitted":
          strip = {
            kind: "changes",
            text: `${plural(b.files, "uncommitted file")}`,
            diff: true,
            prLink,
          };
          break;
        case "unpushed":
          strip = { kind: "info", text: `${plural(b.commits, "commit")} not pushed`, prLink };
          break;
        case "unsubmitted":
          strip = { kind: "info", text: "Pushed, no PR yet", prLink };
          break;
        case "diverged":
          strip = {
            kind: "att",
            text: capitalize(mergeGateLabel(gate?.situation ?? "behind", ctx.base)),
            prLink,
          };
          break;
        case "checks-failing":
          strip = {
            kind: "att",
            text: `${plural(b.checks.length || checks?.failed || 0, "check")} failing`,
            prLink,
          };
          break;
        case "review-unaddressed":
          strip = { kind: "att", text: `${plural(b.count, "review comment")} waiting`, prLink };
          break;
        default:
          // The ladder never delegates the human-owned blockers; a new one
          // reads as its kind until this table learns it.
          strip = { kind: "att", text: b.kind, prLink };
      }
      const label = (() => {
        switch (rung.kind) {
          case "resolve":
            return "Resolve conflicts";
          case "commit-pr":
            return "Commit & open PR";
          case "commit-push":
            return n != null ? `Commit & push to #${n}` : "Commit & push";
          case "commit":
            return "Commit";
          case "push":
            return n != null ? `Push to #${n}` : "Push";
          case "open-pr":
            return "Open PR";
          case "update-branch":
            return "Update branch";
          case "fix-checks":
            return "Fix failing checks";
          case "resolve-comments":
            return `Resolve ${plural(b.kind === "review-unaddressed" ? b.count : 0, "review comment")}`;
        }
      })();
      primary = {
        key: rung.kind,
        label,
        kind: "delegate",
        action: rung.action,
        ...(rung.params ? { params: rung.params } : {}),
        delegation: rung.kind,
      };
      break;
    }
    case "merge":
      strip = { kind: "ready", text: "Ready to merge", prLink };
      primary = extra.canMerge
        ? { key: "merge", label: `Merge PR #${open?.number ?? pr?.number}`, kind: "merge" }
        : github;
      break;
    case "escalate": {
      const b = rung.blocker;
      switch (b.kind) {
        case "review-required":
          strip = { kind: "info", text: "Waiting on a review", prLink };
          break;
        case "draft":
          strip = { kind: "info", text: "Draft on GitHub", prLink };
          break;
        case "review-disputed":
          strip = { kind: "info", text: `${plural(b.count, "thread")} waiting on a reply`, prLink };
          break;
        case "proposal-closed":
          strip = { kind: "neutral", text: `PR #${pr?.number} was closed`, prLink };
          break;
        default:
          strip = { kind: "info", text: b.kind, prLink };
      }
      primary = github;
      break;
    }
    case "ready":
      if (open && gate) {
        strip = {
          kind: "info",
          text: capitalize(mergeGateLabel(gate.situation, ctx.base)),
          prLink,
        };
        primary = github;
      } else {
        strip = { kind: "clean", text: "Nothing to ship yet", sub: `← ${ctx.base}` };
        primary = null;
      }
      break;
    case "landed":
      strip = { kind: "merged", text: `Merged into ${ctx.base}`, prLink };
      primary = { key: "archive", label: "Archive workspace", kind: "archive" };
      break;
  }

  // The overflow: Merge stays reachable from any open state the gate allows
  // (as on the desktop), the manual sheet backs every commit-family playbook,
  // and GitHub is always one tap away once a PR exists. Never Merge while the
  // local tree is conflicted, however green GitHub's gate is: the checkout
  // cannot even be reconciled yet, and that holds while the agent is mid-way
  // through resolving it (the desktop's conflict state offers no merge either).
  const conflicted = (input.git?.files ?? []).some((f) => f.kind === "conflicted");
  const more: ShipAction[] = [];
  if (gate?.mergeAllowed && extra.canMerge && primary?.kind !== "merge" && open && !conflicted) {
    more.push({ key: "merge", label: `Merge PR #${open.number}`, kind: "merge" });
  }
  if (isCommitAction(primary)) {
    more.push({ key: "manual", label: "Write the message yourself", kind: "manual" });
  }
  if (github && primary?.kind !== "github") more.push(github);

  // A playbook in flight: the strip says so, and the footer waits it out.
  if (extra.delegation) {
    return {
      rung,
      strip: { kind: "working", text: delegationLabel(extra.delegation), prLink },
      primary: null,
      more,
    };
  }
  return { rung, strip, primary, more };
}
