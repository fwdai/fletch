// MissionControl/queue.ts — the fleet review queue derivation (PR 3, §1). A pure
// function that composes the app's already-polled state (agents + their diff /
// unseen / PR signals, and workflow runs) into one ordered list of "needs you"
// items. No store, no IPC, no React here — so the ordering, dedupe, signature,
// and dismissal-filter logic are unit-testable in isolation (queue.test.ts) and
// the hook (useReviewQueue) is a thin adapter over this.

import type {
  AgentRecord,
  PrChecks,
  PrComments,
  PrState,
  ShortStats,
  WfPausedReason,
  WfRun,
} from "@/api";

/** Why an item is in the queue. One card can carry several (an agent with both
 *  unseen results and a failing PR is ONE item with both reasons — see the
 *  dedupe in buildReviewQueue). */
export type ReviewReason =
  | "workflow-approval"
  | "workflow-conflict"
  | "unseen-results"
  | "checks-failing"
  | "unresolved-comments";

/** Future-PR slot (§1): a "base moved under this agent" signal. Designed now so
 *  the card can render a staleness chip without a type/layout change when a later
 *  PR feeds it; nothing populates it in this PR (backend deliberately deferred),
 *  so it is always `null` today and the chip never renders. */
export interface Staleness {
  /** The base branch the agent has fallen behind. */
  base: string;
  /** How many commits the base has moved ahead by. */
  behind: number;
}

export interface ReviewItem {
  /** Stable id — `wf:<runId>` for a workflow item, `agent:<agentId>` for an
   *  agent item. Drives keyboard focus and dismissal. */
  id: string;
  kind: "workflow" | "agent";
  /** Ordering bucket (see BUCKET). Lower = more decidable = higher in the queue. */
  bucket: number;
  /** Fingerprint of the item's *volatile* signals. Dismissal is keyed on this,
   *  so a dismissed item returns the moment its signature changes (a new turn,
   *  a new diff, a CI pass/fail flip). */
  signature: string;
  /** Deterministic within-bucket tiebreak — most recent activity first. */
  activityAt: number;
  /** Who: the agent / run display name. */
  title: string;
  /** What (the one-line brief): the session's task text, first line only. */
  goal: string;
  reasons: ReviewReason[];

  // ── agent items ──
  agent?: AgentRecord;

  // ── workflow items ──
  runId?: string;
  pausedReason?: WfPausedReason;

  // ── evidence (all optional — omit when unknown; never fake a zero) ──
  diff?: ShortStats;
  pr?: { number: number; url: string };
  checks?: PrChecks;
  unresolvedComments?: number;
  staleness?: Staleness | null;
}

/** Ordering buckets, most-decidable-first. Rationale: an item is ranked by its
 *  most-decidable reason, so a card with evidence you can act on in one gesture
 *  floats above one that just needs a look.
 *   0 workflow approval — a dedicated evidence surface + one-click promote.
 *   1 workflow conflict — a clear decision with a defined action.
 *   2 PR items (failing checks / unresolved threads) — CI evidence is present.
 *   3 plain unseen-diff agent items — a turn landed with changes to look at. */
export const BUCKET = {
  workflowApproval: 0,
  workflowConflict: 1,
  pr: 2,
  unseen: 3,
} as const;

export interface QueueInput {
  agents: readonly AgentRecord[];
  gitShortstats: Record<string, ShortStats>;
  unseenResults: Record<string, boolean>;
  prStates: Record<string, PrState | null>;
  prChecks: Record<string, PrChecks | null>;
  prComments: Record<string, PrComments | null>;
  runs: readonly WfRun[];
  /** Item id → signal signature it was dismissed at. */
  dismissed: Record<string, string>;
}

/** First non-empty line of a brief, trimmed. Empty string when there's none —
 *  the card falls back to a placeholder rather than showing whitespace. */
function firstLine(s: string | null | undefined): string {
  if (!s) return "";
  for (const raw of s.split("\n")) {
    const line = raw.trim();
    if (line) return line;
  }
  return "";
}

/** Epoch ms from an ISO created_at, or 0 when unparseable — a stable tiebreak
 *  key for agent items (which carry no updated_at). */
function parseCreated(iso: string): number {
  const t = new Date(iso).getTime();
  return Number.isNaN(t) ? 0 : t;
}

/** The agent item's signature: only the volatile review signals, so dismissing
 *  it holds until one of them changes. */
function agentSignature(p: {
  unseen: boolean;
  stats: ShortStats | undefined;
  checks: PrChecks | null;
  unresolved: number;
}): string {
  const d = p.stats ? `${p.stats.additions}/${p.stats.deletions}/${p.stats.file_count}` : "-";
  const c = p.checks ? `${p.checks.rollup}:${p.checks.failed}` : "-";
  return `u${p.unseen ? 1 : 0}|d${d}|c${c}|r${p.unresolved}`;
}

/** Compose the fleet review queue from current app state. Pure and synchronous:
 *  it never blocks on evidence (workflow approvals rank top because they carry a
 *  dedicated review surface, not because their evidence has finished loading). */
export function buildReviewQueue(input: QueueInput): ReviewItem[] {
  const items: ReviewItem[] = [];

  // ── workflow runs paused on a human decision (§1: approval or conflict) ──
  for (const run of input.runs) {
    if (run.status !== "paused") continue;
    if (run.paused_reason !== "approval" && run.paused_reason !== "conflict") continue;
    const approval = run.paused_reason === "approval";
    items.push({
      id: `wf:${run.id}`,
      kind: "workflow",
      bucket: approval ? BUCKET.workflowApproval : BUCKET.workflowConflict,
      // Status + reason: dismissed until the run moves off this pause.
      signature: `${run.status}:${run.paused_reason}`,
      activityAt: run.updated_at,
      title: run.name,
      goal: firstLine(run.task) || run.name,
      reasons: [approval ? "workflow-approval" : "workflow-conflict"],
      runId: run.id,
      pausedReason: run.paused_reason,
    });
  }

  // ── agents (one card per agent; multiple reasons dedupe onto its chips) ──
  for (const agent of input.agents) {
    const stats = input.gitShortstats[agent.id];
    const hasDiff = !!stats && (stats.additions > 0 || stats.deletions > 0);
    const unseen = input.unseenResults[agent.id] ?? false;
    const pr = input.prStates[agent.id] ?? null;
    const checks = input.prChecks[agent.id] ?? null;
    const comments = input.prComments[agent.id] ?? null;
    const prOpen = pr?.state === "open";
    const unresolved = comments?.unresolved.length ?? 0;

    const reasons: ReviewReason[] = [];
    // Ad-hoc: a turn landed while you weren't looking and left changes behind.
    if (agent.status === "idle" && unseen && hasDiff) reasons.push("unseen-results");
    // PR signals only count against an open PR — a merged/closed PR's stale
    // rollup must never nag.
    if (prOpen && checks?.rollup === "failing") reasons.push("checks-failing");
    if (prOpen && unresolved > 0) reasons.push("unresolved-comments");
    if (reasons.length === 0) continue;

    const isPr = reasons.includes("checks-failing") || reasons.includes("unresolved-comments");
    items.push({
      id: `agent:${agent.id}`,
      kind: "agent",
      bucket: isPr ? BUCKET.pr : BUCKET.unseen,
      signature: agentSignature({ unseen, stats, checks, unresolved }),
      activityAt: parseCreated(agent.created_at),
      title: agent.name,
      goal: firstLine(agent.task) || "—",
      reasons,
      agent,
      diff: hasDiff ? stats : undefined,
      pr: prOpen && pr ? { number: pr.number, url: pr.url } : undefined,
      checks: prOpen && checks ? checks : undefined,
      unresolvedComments: reasons.includes("unresolved-comments") ? unresolved : undefined,
      // Future-PR slot — no signal feeds it yet, so it's always absent today.
      staleness: null,
    });
  }

  // Hide items dismissed at their *current* signature; a signature change (new
  // turn, new diff, CI flip) no longer matches the stored mark, so the item
  // resurfaces on its own.
  const visible = items.filter((it) => input.dismissed[it.id] !== it.signature);

  // Most-decidable-first, then most-recent-first, then id for a stable order.
  visible.sort(
    (a, b) =>
      a.bucket - b.bucket ||
      b.activityAt - a.activityAt ||
      (a.id < b.id ? -1 : a.id > b.id ? 1 : 0),
  );
  return visible;
}
