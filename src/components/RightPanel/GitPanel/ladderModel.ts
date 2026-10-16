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
import { describeMergeGate, type MergeGate } from "@/mergeGate";
import { type Blocker, detectBlockers, nextRung, type Rung } from "@/readiness";
import { plural } from "@/util/format";

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

const TITLE: Record<RungKey, string> = {
  changes: "Changes",
  commits: "Commits",
  pr: "Pull request",
  checks: "Checks",
  review: "Review",
};

/** One row, with the title filled in from its key. */
function row(key: RungKey, rest: Omit<LadderRow, "key" | "title">): LadderRow {
  return { key, title: TITLE[key], ...rest };
}

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

/** The gate of an open PR: GitHub's merge gate when the checks read carries
 *  it, else the PR's own coarse `mergeable` (`describeMergeGate` picks). */
function gateOf(pr: PrState, checks: PrChecks | null, mergeState: MergeState | null): MergeGate {
  return describeMergeGate(mergeState, {
    checksFailed: checks?.required_failing.length ?? 0,
    mergeable: pr.mergeable,
  });
}

/** The pull request row for an open PR. Its own concern is that a proposal
 *  exists, is not a draft, and is in sync with its base; checks and review are
 *  their own rows, so their situations read as a plain open PR here. */
function openPrRow(pr: PrState, gate: MergeGate, base: string, current: boolean): LadderRow {
  const label = `#${pr.number}`;
  switch (gate.situation) {
    case "ready":
      return row("pr", { summary: `${label} · ready to merge`, mark: "done", current });
    case "behind":
      return row("pr", {
        summary: `${label} · behind ${base}`,
        mark: "attention",
        tone: "att",
        current,
      });
    case "conflicts":
      return row("pr", {
        summary: `${label} · conflicts with ${base}`,
        mark: "attention",
        tone: "att",
        current,
      });
    case "draft":
      return row("pr", { summary: `${label} · draft`, mark: "attention", tone: "info", current });
    default:
      return row("pr", { summary: `${label} · open`, mark: "done", current });
  }
}

/** The pull request row for a PR that is no longer open. After a merge, work
 *  since it wants its own proposal (`followUp`): the ladder's next step is a
 *  follow-up PR, which the merged row announces rather than a bare push. */
function closedPrRow(pr: PrState, base: string, current: boolean, followUp: boolean): LadderRow {
  const label = `#${pr.number}`;
  if (pr.state === "merged") {
    return followUp
      ? row("pr", {
          summary: `${label} merged · follow-up ready`,
          mark: "attention",
          tone: "info",
          current,
        })
      : row("pr", {
          summary: `${label} · merged into ${base}`,
          mark: "done",
          tone: "merged",
          current,
        });
  }
  return row("pr", { summary: `${label} · closed`, mark: "attention", tone: "neutral", current });
}

/** The checks row of an open PR. `reading` is the first checks read still in
 *  flight, which shows as running rather than missing. */
function checksRow(checks: PrChecks | null, reading: boolean, current: boolean): LadderRow {
  if (!checks) {
    return reading
      ? row("checks", { summary: "checking…", mark: "running", current })
      : row("checks", { summary: "unavailable", mark: "idle", current: false });
  }
  if (checks.total === 0) {
    return row("checks", { summary: "none configured", mark: "idle", current: false });
  }
  switch (checks.rollup) {
    case "failing":
      return row("checks", {
        summary: `${checks.failed} failing · ${checks.passed} passed`,
        mark: "attention",
        tone: "danger",
        current,
      });
    case "pending":
      return row("checks", {
        summary: `${checks.total - checks.pending} of ${checks.total} done`,
        mark: "running",
        current,
      });
    default:
      return row("checks", { summary: `${checks.passed} passed`, mark: "done", current: false });
  }
}

/** The ladder of a PR the panel can only see from its set entry — another PR
 *  of the checkout than the focused one. The host reads checks for every PR of
 *  the set but threads only for the focused one, and the tree's changes and
 *  commits belong to the branch checked out, so this is the PR and its checks:
 *  what can be known, nothing current to act on. */
export function buildPrLadder(entry: PrSetEntry, base: string): LadderRow[] {
  const { state: pr, checks } = entry;
  if (pr.state !== "open") return [closedPrRow(pr, base, false, false)];
  const gate = gateOf(pr, checks, checks?.merge_state ?? null);
  return [openPrRow(pr, gate, base, false), checksRow(checks, false, false)];
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
  const is = (key: RungKey) => current === key;

  const prOpen = pr?.state === "open";
  const prMerged = pr?.state === "merged";
  const label = pr ? `#${pr.number}` : null;

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
      rows.push(
        row("changes", {
          summary: `${plural(conflicted, "conflict")}${inFiles}${where}`,
          mark: "attention",
          tone: "att",
          current: is("changes"),
        }),
      );
    } else if (files > 0) {
      rows.push(
        row("changes", {
          summary: `${plural(files, "file")}${where}`,
          mark: "attention",
          tone: "warn",
          current: is("changes"),
        }),
      );
    } else {
      // Nothing uncommitted. "Done" only once there is work the tree has
      // produced; a fresh checkout has nothing to have committed.
      const anyWork = git.ahead > 0 || git.unpushed > 0 || pr != null;
      rows.push(
        row("changes", {
          summary: anyWork ? "all committed" : "none",
          mark: anyWork ? "done" : "idle",
          current: false,
        }),
      );
    }
  }

  // ── Commits ────────────────────────────────────────────────────────────────
  {
    const { ahead, unpushed } = git;
    if (prMerged) {
      // `ahead` still counts what landed (the local base is stale until the next
      // fetch); only commits origin lacks are new since the merge.
      rows.push(
        unpushed > 0
          ? row("commits", {
              summary: `${plural(unpushed, "commit")} since ${label} · not pushed`,
              mark: "attention",
              tone: "warn",
              current: is("commits"),
            })
          : row("commits", { summary: `landed in ${label}`, mark: "done", current: false }),
      );
    } else if (unpushed > 0) {
      rows.push(
        row("commits", {
          summary:
            ahead > unpushed
              ? `${ahead} ahead of ${base} · ${unpushed} not pushed`
              : `${plural(unpushed, "commit")} · not pushed`,
          mark: "attention",
          tone: "warn",
          current: is("commits"),
        }),
      );
    } else if (ahead > 0) {
      rows.push(
        row("commits", {
          summary: `${ahead} ahead of ${base} · pushed`,
          mark: "done",
          current: false,
        }),
      );
    } else {
      rows.push(row("commits", { summary: "none", mark: "idle", current: false }));
    }
  }

  // ── Pull request ───────────────────────────────────────────────────────────
  const gate = prOpen && pr ? gateOf(pr, checks, mergeState) : null;
  if (!pr) {
    // "Unsubmitted" is the ladder saying the pushed commits want a proposal.
    const wanted = has("unsubmitted");
    rows.push(
      row("pr", {
        summary: wanted ? "none yet · ready to open" : "none yet",
        mark: wanted ? "attention" : "idle",
        tone: wanted ? "info" : undefined,
        current: is("pr"),
      }),
    );
  } else if (gate) {
    rows.push(openPrRow(pr, gate, base, is("pr")));
  } else {
    rows.push(closedPrRow(pr, base, is("pr"), has("unsubmitted")));
  }

  // Checks and review only exist for an open proposal.
  if (!gate) return rows;

  rows.push(checksRow(checks, mergeState === "unknown", is("checks")));

  // ── Review ─────────────────────────────────────────────────────────────────
  {
    const threads = comments?.unresolved ?? [];
    const ours = threads.filter((t) => t.we_replied_last).length;
    const theirs = threads.length - ours;
    if (theirs > 0) {
      rows.push(
        row("review", {
          summary: `${plural(theirs, "thread")} waiting on you`,
          mark: "attention",
          tone: "att",
          current: is("review"),
        }),
      );
    } else if (ours > 0) {
      rows.push(
        row("review", {
          summary: `${plural(ours, "thread")} awaiting their reply`,
          mark: "attention",
          tone: "info",
          current: is("review"),
        }),
      );
    } else if (gate.situation === "review-required") {
      rows.push(
        row("review", {
          summary: "approval required",
          mark: "attention",
          tone: "att",
          current: is("review"),
        }),
      );
    } else if (comments == null) {
      rows.push(row("review", { summary: "unavailable", mark: "idle", current: false }));
    } else {
      // Nothing waiting on anyone. It reads as done only once the gate is
      // positively open; before that it is simply not in the way.
      const settled = gate.situation === "ready" || gate.situation === "mergeable-soft";
      rows.push(
        row("review", {
          summary: "no open threads",
          mark: settled ? "done" : "idle",
          current: false,
        }),
      );
    }
  }

  return rows;
}
