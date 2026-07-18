// MissionControl/queue.ts — the fleet review queue derivation (PR 3, §1). A pure
// function that composes the app's already-polled state (agents + their diff /
// unseen / PR signals, and workflow runs) into one ordered list of "needs you"
// items. No store, no IPC, no React here — so the ordering, dedupe, signature,
// and dismissal-filter logic are unit-testable in isolation (queue.test.ts) and
// the hook (useReviewQueue) is a thin adapter over this.

import type {
  AgentRecord,
  GitMeta,
  PrChecks,
  PrComments,
  PrState,
  ShortStats,
  VerificationReport,
  WfPausedReason,
  WfRun,
} from "@/api";

/** A card's tests-evidence chip state, derived from a turn-end
 *  [`VerificationReport`]. Only ever a definitive verdict — `undefined` while
 *  unknown/running/skipped, so the card never shows a fake state. */
export type TestsEvidence = "passed" | "failed";

/** Why an item is in the queue. One card can carry several (an agent with both
 *  unseen results and a failing PR is ONE item with both reasons — see the
 *  dedupe in buildReviewQueue). */
export type ReviewReason =
  | "workflow-approval"
  | "workflow-conflict"
  | "unseen-results"
  | "checks-failing"
  | "unresolved-comments";

/** A "base moved under this agent" signal — quiet information, not an alarm (a
 *  moved base is normal in a parallel fleet). Fed from the fleet-wide `gitMeta`
 *  poll; rendered as a muted chip on every card state, and on the sidebar row. */
export interface Staleness {
  /** The base branch the agent has fallen behind. */
  base: string;
  /** How many commits the base has moved ahead by. */
  behind: number;
  /** The checkout the signal comes from — `undefined` for the primary, the
   *  subdir for a secondary (so the chip can name it). */
  repo?: string;
}

/** One advisory file-overlap hint: another agent on the same repo is touching
 *  `count` of the same files. Never a warning — just a heads-up that two agents
 *  may conflict. Advisory only; no gating anywhere (docs/multi-repo-followups.md
 *  §2 defers cross-repo merge gating). */
export interface OverlapHint {
  agentName: string;
  count: number;
}

/** The merge fan-out payload (§3): a sibling's PR merged and moved the base, so
 *  N other agents on the same repo are now behind. Carries the moved base, the
 *  merged PR (the "what"), and the affected agents for the one-gesture
 *  "Update all" action. */
export interface FanoutAgent {
  agentId: string;
  name: string;
  behind: number;
  /** Secondary-repo checkout to update; undefined = the primary. */
  subdir?: string;
}
export interface FanoutInfo {
  base: string;
  merged: { title: string; number: number; url: string };
  agents: FanoutAgent[];
}

export interface ReviewItem {
  /** Stable id — `wf:<runId>` for a workflow item, `agent:<agentId>` for an
   *  agent item, `fanout:<repo>:<base>` for a merge fan-out. Drives keyboard
   *  focus and dismissal. */
  id: string;
  kind: "workflow" | "agent" | "fanout";
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
  /** The repo the shown PR lives in — `undefined` for the primary, the subdir
   *  for a secondary. Actions must act on THIS repo's state, not the primary's. */
  prSubdir?: string;
  checks?: PrChecks;
  unresolvedComments?: number;
  /** Turn-end verification verdict (opt-in per project) — a quiet tests chip.
   *  Omitted when unknown/running/no-tests, so it decorates an existing card
   *  and never fakes a state. */
  tests?: TestsEvidence;
  staleness?: Staleness | null;
  /** Advisory overlap hints — other agents on the same repo touching some of
   *  the same files. Omitted when there are none. */
  overlaps?: OverlapHint[];

  // ── fan-out items ──
  fanout?: FanoutInfo;
}

/** Ordering buckets, most-decidable-first. Rationale: an item is ranked by its
 *  most-decidable reason, so a card with evidence you can act on in one gesture
 *  floats above one that just needs a look.
 *   0 workflow approval — a dedicated evidence surface + one-click promote.
 *   1 workflow conflict — a clear decision with a defined action.
 *   2 merge fan-out — one gesture ("Update all") clears a moved base for many.
 *   3 PR items (failing checks / unresolved threads) — CI evidence is present.
 *   4 plain unseen-diff agent items — a turn landed with changes to look at. */
export const BUCKET = {
  workflowApproval: 0,
  workflowConflict: 1,
  fanout: 2,
  pr: 3,
  unseen: 4,
} as const;

export interface QueueInput {
  agents: readonly AgentRecord[];
  gitShortstats: Record<string, ShortStats>;
  /** Per-checkout base staleness + changed-file paths, keyed by `gitKey`. */
  gitMeta: Record<string, GitMeta>;
  unseenResults: Record<string, boolean>;
  prStates: Record<string, PrState | null>;
  prChecks: Record<string, PrChecks | null>;
  prComments: Record<string, PrComments | null>;
  /** Latest turn-end verification report per agent (keyed by agent_id). Absent
   *  = never verified. */
  verificationReports: Record<string, VerificationReport>;
  runs: readonly WfRun[];
  /** Item id → signal signature it was dismissed at. */
  dismissed: Record<string, string>;
}

/** The definitive tests verdict from a turn-end verification, or `undefined`
 *  when there's nothing to show (no report, or its `test` check never ran).
 *  A failing/timed-out/setup-failed test all read as `"failed"` — tests didn't
 *  pass; a `skipped` test (no command) is not a verdict. */
function testsEvidence(report: VerificationReport | undefined): TestsEvidence | undefined {
  const test = report?.checks.find((c) => c.name === "test");
  if (!test || test.outcome === "skipped") return undefined;
  return test.outcome === "passed" ? "passed" : "failed";
}

/** Per-repo map key mirroring `store/git.ts::gitKey` — plain agent id for the
 *  primary repo, `agentId::subdir` for a secondary. Inlined (not imported) to
 *  keep this selector free of the store, per the module contract above. */
function repoKey(agentId: string, subdir?: string): string {
  return subdir ? `${agentId}::${subdir}` : agentId;
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

/** One repo's open-PR signals for an agent. The PR maps are keyed `agentId`
 *  for the primary repo and `agentId::subdir` for secondaries (store/git.ts
 *  `gitKey`) — a multi-repo agent's failing check must surface no matter which
 *  repo it's on. */
interface PrSignal {
  /** "" for the primary repo, the subdir for a secondary. */
  repo: string;
  pr: PrState;
  checks: PrChecks | null;
  unresolved: number;
}

/** Collect the agent's open-PR signals across every repo key, primary first
 *  then secondaries in stable (sorted) order. */
function collectPrSignals(agentId: string, input: QueueInput): PrSignal[] {
  const prefix = `${agentId}::`;
  const keys = [
    agentId,
    ...Object.keys(input.prStates)
      .filter((k) => k.startsWith(prefix))
      .sort(),
  ];
  const out: PrSignal[] = [];
  for (const key of keys) {
    const pr = input.prStates[key] ?? null;
    // Signals only count against an open PR — a merged/closed PR's stale
    // rollup must never nag.
    if (pr?.state !== "open") continue;
    out.push({
      repo: key === agentId ? "" : key.slice(prefix.length),
      pr,
      checks: input.prChecks[key] ?? null,
      unresolved: (input.prComments[key] ?? null)?.unresolved.length ?? 0,
    });
  }
  return out;
}

/** The agent item's signature: only the volatile review signals (across every
 *  repo's PR), so dismissing it holds until one of them changes. */
function agentSignature(p: {
  unseen: boolean;
  stats: ShortStats | undefined;
  signals: PrSignal[];
  tests: TestsEvidence | undefined;
}): string {
  const d = p.stats ? `${p.stats.additions}/${p.stats.deletions}/${p.stats.file_count}` : "-";
  const c = p.signals
    .map((s) => {
      const checks = s.checks ? `${s.checks.rollup}:${s.checks.failed}` : "-";
      return `${s.repo}=${checks}:${s.unresolved}`;
    })
    .join(",");
  // Include the tests verdict so a fresh verification resurfaces a dismissed
  // card (a pass→fail flip, or the first result landing after dismissal).
  return `u${p.unseen ? 1 : 0}|d${d}|c${c || "-"}|t${p.tests ?? "-"}`;
}

/** The agent's stalest checkout vs its base, or null when no base has moved
 *  ahead (or all are unknown). Scans the primary AND every secondary key
 *  (`agentId` / `agentId::subdir` — same convention as the PR maps): a stale
 *  secondary must surface even when the primary is fresh. `behind === null` =
 *  base tip couldn't be resolved (no GitHub / no fetch) → render nothing,
 *  never a zero. */
function stalenessOf(agentId: string, input: QueueInput): Staleness | null {
  const prefix = `${agentId}::`;
  const keys = [
    agentId,
    ...Object.keys(input.gitMeta)
      .filter((k) => k.startsWith(prefix))
      .sort(),
  ];
  let worst: Staleness | null = null;
  for (const key of keys) {
    const meta = input.gitMeta[key];
    if (!meta || meta.behind == null || meta.behind <= 0) continue;
    if (!worst || meta.behind > worst.behind) {
      worst = {
        base: meta.base,
        behind: meta.behind,
        repo: key === agentId ? undefined : key.slice(prefix.length),
      };
    }
  }
  return worst;
}

/** Pairwise file-set overlaps among agents sharing a repo. Two agents "overlap"
 *  when checkouts of the same source repo touch ≥1 of the same file paths; the
 *  hint names the other agent and the shared-file count. EVERY checkout counts —
 *  primary or secondary (`agentId::subdir` gitMeta keys) — since a same-repo
 *  conflict is just as real in a sibling checkout. A pair overlapping in more
 *  than one shared repo merges into one hint with the summed count. Advisory
 *  only — never gates anything. Returns agentId → hints (each side of a pair
 *  gets one). */
function computeOverlaps(input: QueueInput): Record<string, OverlapHint[]> {
  // Every checkout with a non-empty file set, grouped by its source repo path.
  const byRepo = new Map<string, { agent: AgentRecord; files: Set<string> }[]>();
  for (const agent of input.agents) {
    agent.repos.forEach((repo, i) => {
      if (!repo.repo_path) return;
      const key = repoKey(agent.id, i === 0 ? undefined : repo.subdir);
      const files = input.gitMeta[key]?.files ?? [];
      if (files.length === 0) return;
      const entry = { agent, files: new Set(files) };
      const group = byRepo.get(repo.repo_path);
      if (group) group.push(entry);
      else byRepo.set(repo.repo_path, [entry]);
    });
  }

  // agentId → (other agent's name → shared-file count), merged across repos.
  const counts = new Map<string, Map<string, number>>();
  const record = (id: string, otherName: string, count: number) => {
    const per = counts.get(id) ?? new Map<string, number>();
    per.set(otherName, (per.get(otherName) ?? 0) + count);
    counts.set(id, per);
  };
  for (const group of byRepo.values()) {
    for (let i = 0; i < group.length; i++) {
      for (let j = i + 1; j < group.length; j++) {
        const a = group[i];
        const b = group[j];
        if (a.agent.id === b.agent.id) continue;
        let count = 0;
        for (const f of a.files) if (b.files.has(f)) count++;
        if (count === 0) continue;
        record(a.agent.id, b.agent.name, count);
        record(b.agent.id, a.agent.name, count);
      }
    }
  }

  const out: Record<string, OverlapHint[]> = {};
  for (const [id, per] of counts) {
    out[id] = [...per].map(([agentName, count]) => ({ agentName, count }));
  }
  return out;
}

/** Can this agent act on an "Update all" delegation? Idle/running/spawning
 *  agents can (running ones queue the trigger); stopped/errored ones can't. */
function canDelegate(agent: AgentRecord): boolean {
  return agent.status === "idle" || agent.status === "running" || agent.status === "spawning";
}

/** Build the merge fan-out items (§3): for each repo where a sibling's PR has
 *  merged AND other agents on that repo are now behind the base, one actionable
 *  card whose "Update all" delegates `update-branch` to every affected agent.
 *  Pure over the current snapshot — the item disappears on its own once the
 *  affected agents catch up (behind → 0). */
function buildFanoutItems(input: QueueInput): ReviewItem[] {
  interface RepoEntry {
    agent: AgentRecord;
    subdir?: string;
    base: string;
    pr: PrState | null;
    behind: number | null;
  }
  // Group every agent-repo by (source repo path, base branch).
  const groups = new Map<string, RepoEntry[]>();
  for (const agent of input.agents) {
    agent.repos.forEach((repo, i) => {
      const key = repoKey(agent.id, i === 0 ? undefined : repo.subdir);
      const base = repo.parent_branch || "main";
      const groupKey = `${repo.repo_path}\u0000${base}`;
      const entry: RepoEntry = {
        agent,
        subdir: i === 0 ? undefined : repo.subdir,
        base,
        pr: input.prStates[key] ?? null,
        behind: input.gitMeta[key]?.behind ?? null,
      };
      const group = groups.get(groupKey);
      if (group) group.push(entry);
      else groups.set(groupKey, [entry]);
    });
  }

  const items: ReviewItem[] = [];
  for (const [groupKey, entries] of groups) {
    // The most recently merged PR in the group (highest number) is the "what".
    const merged = entries
      .filter((e) => e.pr?.state === "merged")
      .sort((a, b) => (b.pr?.number ?? 0) - (a.pr?.number ?? 0))[0];
    if (!merged?.pr) continue;
    // Agents on this repo now behind the moved base — the merged agent itself
    // is naturally excluded (its HEAD is the base), and only actionable agents
    // are listed so "Update all" never dispatches to a stopped/errored one.
    const affected: FanoutAgent[] = entries
      .filter(
        (e) =>
          e.agent.id !== merged.agent.id &&
          e.behind != null &&
          e.behind > 0 &&
          canDelegate(e.agent),
      )
      .map((e) => ({
        agentId: e.agent.id,
        name: e.agent.name,
        behind: e.behind as number,
        subdir: e.subdir,
      }))
      .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
    if (affected.length === 0) continue;

    const base = merged.base;
    const repoPath = groupKey.slice(0, groupKey.indexOf("\u0000"));
    const pr = merged.pr;
    items.push({
      id: `fanout:${repoPath}:${base}`,
      kind: "fanout",
      bucket: BUCKET.fanout,
      // Merge identity + the affected set: a new merge, or an agent catching up
      // (behind change) / dropping out, moves the signature so a dismissal
      // expires exactly when the situation actually changes.
      signature: `${pr.number}|${affected.map((a) => `${a.agentId}:${a.behind}`).join(",")}`,
      // No natural timestamp; the most recent merge (highest PR number) floats
      // its repo's fan-out to the top of the bucket. Pure + stable.
      activityAt: pr.number,
      title: pr.title || `PR #${pr.number}`,
      goal: `${affected.length} ${affected.length === 1 ? "agent is" : "agents are"} now behind ${base}`,
      reasons: [],
      fanout: {
        base,
        merged: { title: pr.title || `PR #${pr.number}`, number: pr.number, url: pr.url },
        agents: affected,
      },
    });
  }
  return items;
}

/** Compose the fleet review queue from current app state. Pure and synchronous:
 *  it never blocks on evidence (workflow approvals rank top because they carry a
 *  dedicated review surface, not because their evidence has finished loading). */
export function buildReviewQueue(input: QueueInput): ReviewItem[] {
  const items: ReviewItem[] = [];
  const overlaps = computeOverlaps(input);

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
    const signals = collectPrSignals(agent.id, input);
    const failing = signals.filter((s) => s.checks?.rollup === "failing");
    const unresolved = signals.reduce((n, s) => n + s.unresolved, 0);
    const tests = testsEvidence(input.verificationReports[agent.id]);

    const reasons: ReviewReason[] = [];
    // Ad-hoc: a turn landed while you weren't looking and left changes behind.
    if (agent.status === "idle" && unseen && hasDiff) reasons.push("unseen-results");
    if (failing.length > 0) reasons.push("checks-failing");
    if (unresolved > 0) reasons.push("unresolved-comments");
    if (reasons.length === 0) continue;

    // The evidence chips show one PR: the one carrying the issue (a failing
    // repo first, then one with unresolved threads, then the primary).
    const shown = failing[0] ?? signals.find((s) => s.unresolved > 0) ?? signals[0];
    const isPr = reasons.includes("checks-failing") || reasons.includes("unresolved-comments");
    items.push({
      id: `agent:${agent.id}`,
      kind: "agent",
      bucket: isPr ? BUCKET.pr : BUCKET.unseen,
      signature: agentSignature({ unseen, stats, signals, tests }),
      activityAt: parseCreated(agent.created_at),
      title: agent.name,
      goal: firstLine(agent.task) || "—",
      reasons,
      agent,
      diff: hasDiff ? stats : undefined,
      pr: shown ? { number: shown.pr.number, url: shown.pr.url } : undefined,
      prSubdir: shown?.repo ? shown.repo : undefined,
      checks: shown?.checks ?? undefined,
      unresolvedComments: unresolved > 0 ? unresolved : undefined,
      // Decoration on an existing card (like staleness/overlaps below): the
      // tests chip never creates a card, only annotates one.
      tests,
      // Always-visible signals (§2/§4): the base-moved chip and overlap hints
      // decorate an existing card in every panel state — they never create one.
      staleness: stalenessOf(agent.id, input),
      overlaps: overlaps[agent.id],
    });
  }

  // ── merge fan-out (§3): a sibling merged, siblings on the repo are behind ──
  items.push(...buildFanoutItems(input));

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
