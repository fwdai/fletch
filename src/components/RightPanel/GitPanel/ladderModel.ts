// ── The ladder: one lane's way to landing, as rows ────────────────────────────
// `readiness.ts` owns the ONE ordering of what stands between work and landing
// and what to do next. This module renders that ordering as the Git panel's
// body: a fixed sequence of stages — changes, commits, pull request, checks,
// review — each with a mark (done / attention / running / not yet) and a
// one-line summary. Rows never appear or vanish with state; they change their
// mark. The row the ladder's next step is about is `current`, so the body and
// the footer's primary action always point at the same thing.
//
// Pure and store-free, like `readiness.ts`, so every state has a test.

import type { GitState, MergeState, PrChecks, PrComments, PrSetEntry, PrState } from "@/api";
import { describeMergeGate, type MergeGateSituation } from "@/mergeGate";
import { type Blocker, detectBlockers, nextRung, type Rung } from "@/readiness";

export type RungKey = "changes" | "commits" | "pr" | "checks" | "review";

/** How a stage reads at a glance. `attention` carries a `tone`. */
export type RungMark = "done" | "idle" | "running" | "attention";

/** The attention colours, named for what they mean here rather than for the
 *  token, so a row's meaning and its colour are decided in one place. */
export type RungTone =
  | "warn" // uncommitted / unpushed: your work, not yet safe
  | "att" // out of sync / waiting on you: fixable, not fatal
  | "danger" // failing checks
  | "info" // a step that hasn't happened yet but could (no PR, draft)
  | "merged" // landed
  | "neutral"; // closed

export interface LadderRow {
  key: RungKey;
  title: string;
  summary: string;
  mark: RungMark;
  tone?: RungTone;
  /** The ladder's next step is about this row. */
  current: boolean;
}

export interface LadderInput {
  git: GitState | null;
  pr: PrState | null;
  /** Null = confirmed unavailable. */
  checks: PrChecks | null;
  /** `"unknown"` while the first checks read is in flight (see
   *  `useGitPanelData`), null with no checks data. */
  mergeState: MergeState | null;
  comments: PrComments | null;
  base: string;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** Which row a blocker lives on. */
function rowOfBlocker(kind: Blocker["kind"]): RungKey {
  switch (kind) {
    case "conflicted":
    case "uncommitted":
      return "changes";
    case "unpushed":
      return "commits";
    case "unsubmitted":
    case "proposal-closed":
    case "diverged":
    case "draft":
      return "pr";
    case "checks-failing":
      return "checks";
    case "review-unaddressed":
    case "review-disputed":
    case "review-required":
      return "review";
  }
}

/** Which row the ladder's next step is about, or null when there is nothing
 *  to do or nothing to trust yet. */
export function currentRow(rung: Rung, pr: PrState | null): RungKey | null {
  switch (rung.do) {
    case "delegate":
    case "escalate":
      return rowOfBlocker(rung.blocker.kind);
    case "merge":
    case "landed":
      return "pr";
    case "ready":
      // Nothing blocking: with an open PR the step is the merge decision; with
      // no PR there is simply nothing in flight.
      return pr?.state === "open" ? "pr" : null;
    case "wait":
      return rung.why === "gate-computing" ? "checks" : null;
  }
}

/** The PR row's own concern: that a proposal exists, is not a draft, and is in
 *  sync with its base. Checks and review are their own rows, so their
 *  situations read as a plain open PR here. */
function prSummary(situation: MergeGateSituation, base: string): string {
  switch (situation) {
    case "ready":
      return "ready to merge";
    case "behind":
      return `behind ${base}`;
    case "conflicts":
      return `conflicts with ${base}`;
    case "draft":
      return "draft";
    default:
      return "open";
  }
}

/** The ladder of a PR the panel can only see from its set entry — another PR
 *  of the checkout than the focused one. The host reads checks for every PR of
 *  the set but threads only for the focused one, and the tree's changes and
 *  commits belong to the branch checked out, so this is the PR and its checks:
 *  what can be known, nothing current to act on. */
export function buildPrLadder(entry: PrSetEntry, base: string): LadderRow[] {
  const { state: pr, checks } = entry;
  const label = `#${pr.number}`;
  if (pr.state === "merged") {
    return [
      {
        key: "pr",
        title: "Pull request",
        summary: `${label} · merged into ${base}`,
        mark: "done",
        tone: "merged",
        current: false,
      },
    ];
  }
  if (pr.state === "closed") {
    return [
      {
        key: "pr",
        title: "Pull request",
        summary: `${label} · closed`,
        mark: "attention",
        tone: "neutral",
        current: false,
      },
    ];
  }
  const gate = describeMergeGate(checks?.merge_state ?? null, {
    checksFailed: checks?.required_failing.length ?? 0,
    mergeable: pr.mergeable,
  });
  const summary = `${label} · ${prSummary(gate.situation, base)}`;
  const prRow: LadderRow =
    gate.situation === "behind" || gate.situation === "conflicts"
      ? {
          key: "pr",
          title: "Pull request",
          summary,
          mark: "attention",
          tone: "att",
          current: false,
        }
      : gate.situation === "draft"
        ? {
            key: "pr",
            title: "Pull request",
            summary,
            mark: "attention",
            tone: "info",
            current: false,
          }
        : { key: "pr", title: "Pull request", summary, mark: "done", current: false };
  const checksRow: LadderRow = !checks
    ? { key: "checks", title: "Checks", summary: "unavailable", mark: "idle", current: false }
    : checks.total === 0
      ? { key: "checks", title: "Checks", summary: "none configured", mark: "idle", current: false }
      : checks.rollup === "failing"
        ? {
            key: "checks",
            title: "Checks",
            summary: `${checks.failed} failing · ${checks.passed} passed`,
            mark: "attention",
            tone: "danger",
            current: false,
          }
        : checks.rollup === "pending"
          ? {
              key: "checks",
              title: "Checks",
              summary: `${checks.total - checks.pending} of ${checks.total} done`,
              mark: "running",
              current: false,
            }
          : {
              key: "checks",
              title: "Checks",
              summary: `${checks.passed} passed`,
              mark: "done",
              current: false,
            };
  return [prRow, checksRow];
}

export function buildLadder(input: LadderInput): LadderRow[] {
  const { git, pr, checks, mergeState, comments, base } = input;
  if (!git) return [];

  const blockers = detectBlockers({ git, pr, checks, comments });
  const has = (kind: Blocker["kind"]) => blockers.some((b) => b.kind === kind);
  // The commit mode only shapes the delegation's kind, never which row it is
  // about, so any mode gives the same row.
  const current = currentRow(
    nextRung({ git, pr, checks, comments }, { base, commitMode: "commit" }),
    pr,
  );

  const prOpen = pr?.state === "open";
  const prMerged = pr?.state === "merged";
  const label = pr ? `#${pr.number}` : null;
  const gate = prOpen
    ? describeMergeGate(mergeState, {
        checksFailed: checks?.required_failing.length ?? 0,
        mergeable: pr.mergeable,
      })
    : null;

  const rows: LadderRow[] = [];

  // ── Changes ────────────────────────────────────────────────────────────────
  {
    const files = git.files.length;
    const conflicted = git.files.filter((f) => f.kind === "conflicted").length;
    // The tree is on another branch than this PR's: say so, since a commit
    // from here lands there, not on the PR.
    const where = pr?.branch && git.branch && pr.branch !== git.branch ? ` on ${git.branch}` : "";
    if (conflicted > 0) {
      const inFiles = conflicted === files ? "" : ` in ${plural(files, "file")}`;
      rows.push({
        key: "changes",
        title: "Changes",
        summary: `${plural(conflicted, "conflict")}${inFiles}${where}`,
        mark: "attention",
        tone: "att",
        current: current === "changes",
      });
    } else if (files > 0) {
      rows.push({
        key: "changes",
        title: "Changes",
        summary: `${plural(files, "file")}${where}`,
        mark: "attention",
        tone: "warn",
        current: current === "changes",
      });
    } else {
      // Nothing uncommitted. "Done" only once there is work the tree has
      // produced; a fresh checkout has nothing to have committed.
      const anyWork = git.ahead > 0 || git.unpushed > 0 || pr != null;
      rows.push({
        key: "changes",
        title: "Changes",
        summary: anyWork ? "all committed" : "none",
        mark: anyWork ? "done" : "idle",
        current: false,
      });
    }
  }

  // ── Commits ────────────────────────────────────────────────────────────────
  {
    const { ahead, unpushed } = git;
    const isCurrent = current === "commits";
    if (prMerged) {
      // `ahead` still counts what landed (the local base is stale until the next
      // fetch); only commits origin lacks are new since the merge.
      rows.push(
        unpushed > 0
          ? {
              key: "commits",
              title: "Commits",
              summary: `${plural(unpushed, "commit")} since ${label} · not pushed`,
              mark: "attention",
              tone: "warn",
              current: isCurrent,
            }
          : {
              key: "commits",
              title: "Commits",
              summary: `landed in ${label}`,
              mark: "done",
              current: false,
            },
      );
    } else if (unpushed > 0) {
      rows.push({
        key: "commits",
        title: "Commits",
        summary:
          ahead > unpushed
            ? `${ahead} ahead of ${base} · ${unpushed} not pushed`
            : `${plural(unpushed, "commit")} · not pushed`,
        mark: "attention",
        tone: "warn",
        current: isCurrent,
      });
    } else if (ahead > 0) {
      rows.push({
        key: "commits",
        title: "Commits",
        summary: `${ahead} ahead of ${base} · pushed`,
        mark: "done",
        current: false,
      });
    } else {
      rows.push({
        key: "commits",
        title: "Commits",
        summary: "none",
        mark: "idle",
        current: false,
      });
    }
  }

  // ── Pull request ───────────────────────────────────────────────────────────
  if (!pr) {
    // "Unsubmitted" is the ladder saying the pushed commits want a proposal.
    const wanted = has("unsubmitted");
    rows.push({
      key: "pr",
      title: "Pull request",
      summary: wanted ? "none yet · ready to open" : "none yet",
      mark: wanted ? "attention" : "idle",
      tone: wanted ? "info" : undefined,
      current: current === "pr",
    });
  } else if (prOpen && gate) {
    const summary = `${label} · ${prSummary(gate.situation, base)}`;
    const isCurrent = current === "pr";
    switch (gate.situation) {
      case "behind":
      case "conflicts":
        rows.push({
          key: "pr",
          title: "Pull request",
          summary,
          mark: "attention",
          tone: "att",
          current: isCurrent,
        });
        break;
      case "draft":
        rows.push({
          key: "pr",
          title: "Pull request",
          summary,
          mark: "attention",
          tone: "info",
          current: isCurrent,
        });
        break;
      default:
        rows.push({ key: "pr", title: "Pull request", summary, mark: "done", current: isCurrent });
    }
  } else if (prMerged) {
    // Work since the merge wants its own proposal: the ladder's next step is a
    // follow-up PR, which the merged row announces rather than a bare push.
    const followUp = has("unsubmitted");
    rows.push({
      key: "pr",
      title: "Pull request",
      summary: followUp ? `${label} merged · follow-up ready` : `${label} · merged into ${base}`,
      mark: followUp ? "attention" : "done",
      tone: followUp ? "info" : "merged",
      current: current === "pr",
    });
  } else {
    rows.push({
      key: "pr",
      title: "Pull request",
      summary: `${label} · closed`,
      mark: "attention",
      tone: "neutral",
      current: current === "pr",
    });
  }

  // Checks and review only exist for an open proposal.
  if (!prOpen || !gate) return rows;

  // ── Checks ─────────────────────────────────────────────────────────────────
  {
    const isCurrent = current === "checks";
    if (!checks) {
      rows.push(
        mergeState === "unknown"
          ? {
              key: "checks",
              title: "Checks",
              summary: "checking…",
              mark: "running",
              current: isCurrent,
            }
          : {
              key: "checks",
              title: "Checks",
              summary: "unavailable",
              mark: "idle",
              current: false,
            },
      );
    } else if (checks.total === 0) {
      rows.push({
        key: "checks",
        title: "Checks",
        summary: "none configured",
        mark: "idle",
        current: false,
      });
    } else if (checks.rollup === "failing") {
      rows.push({
        key: "checks",
        title: "Checks",
        summary: `${checks.failed} failing · ${checks.passed} passed`,
        mark: "attention",
        tone: "danger",
        current: isCurrent,
      });
    } else if (checks.rollup === "pending") {
      rows.push({
        key: "checks",
        title: "Checks",
        summary: `${checks.total - checks.pending} of ${checks.total} done`,
        mark: "running",
        current: isCurrent,
      });
    } else {
      rows.push({
        key: "checks",
        title: "Checks",
        summary: `${checks.passed} passed`,
        mark: "done",
        current: false,
      });
    }
  }

  // ── Review ─────────────────────────────────────────────────────────────────
  {
    const isCurrent = current === "review";
    const threads = comments?.unresolved ?? [];
    const ours = threads.filter((t) => t.we_replied_last).length;
    const theirs = threads.length - ours;
    const required = gate.situation === "review-required";
    if (theirs > 0) {
      rows.push({
        key: "review",
        title: "Review",
        summary: `${plural(theirs, "thread")} waiting on you`,
        mark: "attention",
        tone: "att",
        current: isCurrent,
      });
    } else if (ours > 0) {
      rows.push({
        key: "review",
        title: "Review",
        summary: `${plural(ours, "thread")} awaiting their reply`,
        mark: "attention",
        tone: "info",
        current: isCurrent,
      });
    } else if (required) {
      rows.push({
        key: "review",
        title: "Review",
        summary: "approval required",
        mark: "attention",
        tone: "att",
        current: isCurrent,
      });
    } else if (comments == null) {
      rows.push({
        key: "review",
        title: "Review",
        summary: "unavailable",
        mark: "idle",
        current: false,
      });
    } else {
      // Nothing waiting on anyone. It reads as done only once the gate is
      // positively open; before that it is simply not in the way.
      const settled = gate.situation === "ready" || gate.situation === "mergeable-soft";
      rows.push({
        key: "review",
        title: "Review",
        summary: "no open threads",
        mark: settled ? "done" : "idle",
        current: false,
      });
    }
  }

  return rows;
}
