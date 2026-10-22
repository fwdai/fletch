// ── The ladder: one lane's way to landing, as rows ────────────────────────────
// `readiness.ts` owns the ONE ordering of what stands between work and landing
// and what to do next. This module renders that ordering as the Git panel's
// body: a fixed sequence of stages — changes, commits, pull request, checks,
// review — each with a mark (done / attention / running / not yet) and a
// one-line summary. Rows never appear or vanish with state, nor with the lane
// in view: a lane the tree isn't on, or whose threads aren't read, shows the
// same rows marked not-yet and says why. The row the ladder's next step is
// about is `current`, so the body and the footer's primary action always
// point at the same thing.
//
// Pure and store-free, like `readiness.ts`, so every state has a test.

import type { GitState, MergeState, PrChecks, PrComments, PrState, ReviewState } from "@/api";
import { describeMergeGate, type MergeGate } from "@/mergeGate";
import { type Blocker, detectBlockers, nextRung, type Rung } from "@/readiness";
import { formatAge, plural } from "@/util/format";

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
  /** How the lane relates to the checkout. `tree`: its changes and commits are
   *  the working tree's (the focused PR's lane, or the tree's own); otherwise
   *  the tree is on another branch and those rows say so. `focused`: the host
   *  reads its threads and the ladder has a next step; a lane only looked at
   *  has neither. Both default to true. */
  lane?: { tree: boolean; focused: boolean };
  /** The local reviewer's state for the checkout, when the host reported one.
   *  Absent or null: never reviewed (the row then reads from the threads and
   *  the merge gate alone, as before the local reviewer existed). */
  review?: ReviewState | null;
  /** The host can run a local review. With no PR open the Review row is
   *  shown only then — it is the way to ask for one — and only once the tree
   *  has produced work to review. */
  canReview?: boolean;
  /** The clock, for the running timer and the age of a verdict. Passed in so
   *  the model stays pure; absent, those read without a time. */
  nowMs?: number;
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

const idle = (key: RungKey, summary: string): LadderRow =>
  row(key, { summary, mark: "idle", current: false });

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
      : idle("checks", "unavailable");
  }
  if (checks.total === 0) return idle("checks", "none configured");
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

export function buildLadder(input: LadderInput): LadderRow[] {
  const { git, pr, checks, mergeState, comments, base } = input;
  const lane = input.lane ?? { tree: true, focused: true };
  if (!git) return [];

  const blockers = detectBlockers({ git, pr, checks, comments });
  const has = (kind: Blocker["kind"]) => blockers.some((b) => b.kind === kind);
  // The next step belongs to the lane the tree is on; a lane only looked at
  // has none. The commit mode only shapes the delegation's kind, never which
  // row it is about, so any mode gives the same row.
  const current = lane.tree
    ? currentRow(nextRung({ git, pr, checks, comments }, { base, commitMode: "commit" }), pr)
    : null;
  const is = (key: RungKey) => current === key;

  const prOpen = pr?.state === "open";
  const prMerged = pr?.state === "merged";
  const label = pr ? `#${pr.number}` : null;

  const rows: LadderRow[] = [];

  // The tree is on another branch than this PR's: its changes and commits say
  // so, since a commit from here lands there, not on the PR.
  const where = pr?.branch && git.branch && pr.branch !== git.branch ? ` on ${git.branch}` : "";

  // ── Changes ────────────────────────────────────────────────────────────────
  if (!lane.tree) {
    rows.push(idle("changes", git.branch ? `working tree is on ${git.branch}` : "not this lane's"));
  } else {
    const files = git.files.length;
    const conflicted = git.files.filter((f) => f.kind === "conflicted").length;
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
        anyWork
          ? row("changes", { summary: "all committed", mark: "done", current: false })
          : idle("changes", "none"),
      );
    }
  }

  // ── Commits ────────────────────────────────────────────────────────────────
  if (!lane.tree) {
    rows.push(idle("commits", "not checked out"));
  } else {
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
            (ahead > unpushed
              ? `${ahead} ahead of ${base} · ${unpushed} not pushed`
              : `${plural(unpushed, "commit")} · not pushed`) + where,
          mark: "attention",
          tone: "warn",
          current: is("commits"),
        }),
      );
    } else if (ahead > 0) {
      rows.push(
        row("commits", {
          summary: `${ahead} ahead of ${base} · pushed${where}`,
          mark: "done",
          current: false,
        }),
      );
    } else {
      rows.push(idle("commits", "none"));
    }
  }

  // ── Pull request ───────────────────────────────────────────────────────────
  const gate =
    prOpen && pr
      ? describeMergeGate(mergeState, {
          checksFailed: checks?.required_failing.length ?? 0,
          mergeable: pr.mergeable,
        })
      : null;
  if (!pr) {
    // "Unsubmitted" is the ladder saying the pushed commits want a proposal.
    const wanted = lane.tree && has("unsubmitted");
    rows.push(
      wanted
        ? row("pr", {
            summary: "none yet · ready to open",
            mark: "attention",
            tone: "info",
            current: is("pr"),
          })
        : idle("pr", "none yet"),
    );
  } else if (gate) {
    rows.push(openPrRow(pr, gate, base, is("pr")));
  } else {
    rows.push(closedPrRow(pr, base, is("pr"), lane.tree && has("unsubmitted")));
  }

  // Checks exist for an open proposal only. Review does too — unless the local
  // reviewer can run (or has run) on the tree before any proposal exists.
  const review = input.review ?? null;
  const localThreads = (comments?.unresolved ?? []).some((t) => t.source === "local");
  const anyWork = git.files.length > 0 || git.ahead > 0 || git.unpushed > 0;
  const preReview = lane.tree && (review != null || localThreads || (input.canReview && anyWork));
  if (!gate && !preReview) return rows;

  if (gate) rows.push(checksRow(checks, mergeState === "unknown", is("checks")));

  // ── Review ─────────────────────────────────────────────────────────────────
  if (!lane.focused) {
    // Threads are read for the focused PR only.
    rows.push(idle("review", "read for the focused PR"));
    return rows;
  }
  rows.push(reviewRow(input, gate, is("review")));
  return rows;
}

/** `m:ss` for a timer anchored at `since`. */
function elapsed(since: number, now: number): string {
  const s = Math.max(0, Math.floor((now - since) / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** The review row: GitHub's review threads and the local reviewer's findings
 *  on one line, the reviewer's own phase (reading, failed, stale, clean) in
 *  between. Most pressing first — a turn in flight, then threads waiting on
 *  us, then what stops a re-review from being trusted, then the quiet states. */
function reviewRow(input: LadderInput, gate: MergeGate | null, current: boolean): LadderRow {
  const { comments } = input;
  const review = input.review ?? null;
  const name = review?.reviewer?.name ?? "reviewer";
  const threads = comments?.unresolved ?? [];
  const ours = threads.filter((t) => t.we_replied_last).length;
  const theirs = threads.length - ours;
  const local = threads.some((t) => t.source === "local");
  const att = (summary: string, tone: RungTone = "att"): LadderRow =>
    row("review", { summary, mark: "attention", tone, current });

  if (review?.phase === "reviewing") {
    const timer = input.nowMs != null ? ` · ${elapsed(review.since_ms, input.nowMs)}` : "";
    return row("review", { summary: `Reviewing · ${name}${timer}`, mark: "running", current });
  }
  if (theirs > 0) {
    // The local reviewer's findings count by round; GitHub's threads are
    // simply waiting. One line either way, the round said when it applies.
    const noun = local ? "finding" : "thread";
    const round = local && review && review.round > 0 ? ` · round ${review.round}` : "";
    return att(`${plural(theirs, noun)} waiting on you${round}`);
  }
  if (review?.phase === "failed") {
    return att(`Review failed${review.error ? ` · ${review.error}` : ""}`, "danger");
  }
  if (review?.phase === "stale") return att("Stale · code changed since review", "info");
  if (ours > 0) return att(`${plural(ours, "thread")} awaiting their reply`, "info");
  if (gate?.situation === "review-required") return att("approval required");
  if (review?.phase === "clean" || (review?.phase === "findings" && review.open === 0)) {
    const age =
      input.nowMs != null
        ? formatAge(review.since_ms, input.nowMs) === "now"
          ? "just now"
          : `${formatAge(review.since_ms, input.nowMs)} ago`
        : null;
    const fixed = review.resolved > 0 ? ` · ${review.resolved} fixed` : "";
    return row("review", {
      summary: `Clean · ${name}${age ? `, ${age}` : ""}${fixed}`,
      mark: "done",
      current: false,
    });
  }
  if (!gate) return idle("review", "not reviewed");
  if (comments == null) return idle("review", "unavailable");
  // Nothing waiting on anyone. It reads as done only once the gate is
  // positively open; before that it is simply not in the way.
  const settled = gate.situation === "ready" || gate.situation === "mergeable-soft";
  return settled
    ? row("review", { summary: "no open threads", mark: "done", current: false })
    : idle("review", "no open threads");
}
