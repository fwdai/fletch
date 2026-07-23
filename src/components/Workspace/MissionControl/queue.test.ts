import { describe, expect, it } from "vitest";
import type {
  AgentRecord,
  CheckOutcome,
  GitMeta,
  PrChecks,
  PrComments,
  PrState,
  ShortStats,
  VerificationReport,
  WfRun,
} from "@/api";
import { BUCKET, buildReviewQueue, type QueueInput } from "./queue";

// ── fixtures ──────────────────────────────────────────────────────────────────

function agent(over: Partial<AgentRecord> & { id: string }): AgentRecord {
  return {
    project_id: "p1",
    name: over.id,
    provider: "claude",
    repos: [],
    task: "Do the thing",
    status: "idle",
    view: "custom",
    created_at: "2026-07-17T00:00:00.000Z",
    ...over,
  };
}

const stats = (additions: number, deletions: number, file_count = 1): ShortStats => ({
  additions,
  deletions,
  file_count,
});

const openPr = (number = 1): PrState => ({
  number,
  url: `https://gh/pr/${number}`,
  state: "open",
  title: "PR",
  mergeable: "mergeable",
});

function checks(over: Partial<PrChecks>): PrChecks {
  return {
    merge_state: "blocked",
    rollup: "failing",
    total: 3,
    passed: 2,
    failed: 1,
    pending: 0,
    required_failing: ["test"],
    runs: [],
    ...over,
  };
}

const comments = (n: number): PrComments => ({
  unresolved: Array.from({ length: n }, (_, i) => ({
    author: "bot",
    is_bot: true,
    body: `c${i}`,
    path: null,
    line: null,
    url: `u${i}`,
    replies: 0,
  })),
});

function run(over: Partial<WfRun> & { id: string }): WfRun {
  return {
    definition_id: null,
    parent_run_id: null,
    name: over.id,
    spec: {},
    task: "Ship it",
    project_id: "p1",
    repo_path: "/repo",
    run_dir: "/run",
    branch: "wf/x",
    base_sha: "abc",
    status: "paused",
    paused_reason: "approval",
    cursor: null,
    budgets: {},
    spent: {},
    error: null,
    created_at: 1,
    updated_at: 1000,
    ...over,
  };
}

const meta = (over: Partial<GitMeta>): GitMeta => ({
  base: "main",
  behind: null,
  files: [],
  ...over,
});

/** A verification report whose `test` check has the given outcome. */
const report = (testOutcome: CheckOutcome): VerificationReport => ({
  checks: [{ name: "test", command: "npm test", outcome: testOutcome, duration_ms: 1, tail: [] }],
});

function input(over: Partial<QueueInput>): QueueInput {
  return {
    agents: [],
    gitShortstats: {},
    gitMeta: {},
    unseenResults: {},
    prStates: {},
    prChecks: {},
    prComments: {},
    verificationReports: {},
    runs: [],
    dismissed: {},
    ...over,
  };
}

/** A single-repo agent bound to `repoPath`, so fan-out/overlap grouping keys off
 *  a real primary repo. */
function repoAgent(id: string, repoPath: string, over: Partial<AgentRecord> = {}): AgentRecord {
  return agent({
    id,
    repos: [{ repo_path: repoPath, subdir: "", parent_branch: "main" }],
    ...over,
  });
}

// ── tests ───────────────────────────────────────────────────────────────────

describe("buildReviewQueue", () => {
  it("returns nothing for an idle fleet with no signals", () => {
    expect(buildReviewQueue(input({ agents: [agent({ id: "a" })] }))).toEqual([]);
  });

  it("surfaces an idle agent with unseen results AND a nonzero diff", () => {
    const q = buildReviewQueue(
      input({
        agents: [agent({ id: "a" })],
        unseenResults: { a: true },
        gitShortstats: { a: stats(4, 2) },
      }),
    );
    expect(q).toHaveLength(1);
    expect(q[0].id).toBe("agent:a");
    expect(q[0].bucket).toBe(BUCKET.unseen);
    expect(q[0].reasons).toEqual(["unseen-results"]);
    expect(q[0].diff).toEqual(stats(4, 2));
  });

  it("requires all three of idle + unseen + diff", () => {
    // Running (not idle) → excluded.
    expect(
      buildReviewQueue(
        input({
          agents: [agent({ id: "a", status: "running" })],
          unseenResults: { a: true },
          gitShortstats: { a: stats(4, 2) },
        }),
      ),
    ).toEqual([]);
    // Seen (no unseen flag) → excluded.
    expect(
      buildReviewQueue(input({ agents: [agent({ id: "a" })], gitShortstats: { a: stats(4, 2) } })),
    ).toEqual([]);
    // No diff → excluded.
    expect(
      buildReviewQueue(input({ agents: [agent({ id: "a" })], unseenResults: { a: true } })),
    ).toEqual([]);
  });

  it("only counts PR signals against an OPEN pr", () => {
    const merged: PrState = { ...openPr(), state: "merged" };
    const q = buildReviewQueue(
      input({
        agents: [agent({ id: "a" })],
        prStates: { a: merged },
        prChecks: { a: checks({ rollup: "failing" }) },
        prComments: { a: comments(2) },
      }),
    );
    expect(q).toEqual([]);
  });

  it("dedupes one agent's many signals into ONE card with all reasons", () => {
    const q = buildReviewQueue(
      input({
        agents: [agent({ id: "a" })],
        unseenResults: { a: true },
        gitShortstats: { a: stats(1, 1) },
        prStates: { a: openPr(7) },
        prChecks: { a: checks({ rollup: "failing", failed: 2 }) },
        prComments: { a: comments(3) },
      }),
    );
    expect(q).toHaveLength(1);
    expect(q[0].reasons).toEqual(["unseen-results", "checks-failing", "unresolved-comments"]);
    // Ranked by its most-decidable reason (PR), not the plain unseen bucket.
    expect(q[0].bucket).toBe(BUCKET.pr);
    expect(q[0].pr).toEqual({ number: 7, url: "https://gh/pr/7" });
    expect(q[0].unresolvedComments).toBe(3);
  });

  it("surfaces PR signals from a secondary repo key (agentId::subdir)", () => {
    const q = buildReviewQueue(
      input({
        agents: [agent({ id: "a" })],
        // Primary repo: open PR, all green. Secondary repo: failing checks.
        prStates: { a: openPr(1), "a::pkg/api": openPr(2) },
        prChecks: {
          a: checks({ rollup: "passing", failed: 0, required_failing: [] }),
          "a::pkg/api": checks({ rollup: "failing", failed: 1 }),
        },
        prComments: { "a::pkg/api": comments(2) },
      }),
    );
    expect(q).toHaveLength(1);
    expect(q[0].reasons).toEqual(["checks-failing", "unresolved-comments"]);
    // The chips show the PR carrying the issue — the failing secondary — and
    // the item carries its repo so actions act on the same scoped state.
    expect(q[0].pr).toEqual({ number: 2, url: "https://gh/pr/2" });
    expect(q[0].prSubdir).toBe("pkg/api");
    expect(q[0].checks?.rollup).toBe("failing");
    expect(q[0].unresolvedComments).toBe(2);
    // A fix on the secondary changes the signature, so a dismissal expires.
    const fixed = buildReviewQueue(
      input({
        agents: [agent({ id: "a" })],
        prStates: { a: openPr(1), "a::pkg/api": openPr(2) },
        prChecks: {
          a: checks({ rollup: "passing", failed: 0, required_failing: [] }),
          "a::pkg/api": checks({ rollup: "failing", failed: 1 }),
        },
        prComments: { "a::pkg/api": comments(2) },
        dismissed: { "agent:a": q[0].signature },
      }),
    );
    expect(fixed).toEqual([]);
  });

  it("orders approval < conflict < pr < unseen", () => {
    const q = buildReviewQueue(
      input({
        agents: [agent({ id: "unseenAgent" }), agent({ id: "prAgent" })],
        unseenResults: { unseenAgent: true, prAgent: true },
        gitShortstats: { unseenAgent: stats(1, 0), prAgent: stats(2, 0) },
        prStates: { prAgent: openPr(9) },
        prChecks: { prAgent: checks({ rollup: "failing" }) },
        runs: [
          run({ id: "conflict", paused_reason: "conflict" }),
          run({ id: "approval", paused_reason: "approval" }),
        ],
      }),
    );
    expect(q.map((i) => i.id)).toEqual([
      "wf:approval",
      "wf:conflict",
      "agent:prAgent",
      "agent:unseenAgent",
    ]);
  });

  it("ignores workflow runs not paused on approval/conflict", () => {
    const q = buildReviewQueue(
      input({
        runs: [
          run({ id: "q", paused_reason: "question" }),
          run({ id: "b", paused_reason: "budget_exceeded" }),
          run({ id: "r", status: "running", paused_reason: null }),
        ],
      }),
    );
    expect(q).toEqual([]);
  });

  it("tiebreaks a bucket by most-recent activity", () => {
    const q = buildReviewQueue(
      input({
        runs: [
          run({ id: "old", paused_reason: "approval", updated_at: 100 }),
          run({ id: "new", paused_reason: "approval", updated_at: 900 }),
        ],
      }),
    );
    expect(q.map((i) => i.id)).toEqual(["wf:new", "wf:old"]);
  });

  it("hides an item dismissed at its current signature", () => {
    const base = input({
      agents: [agent({ id: "a" })],
      unseenResults: { a: true },
      gitShortstats: { a: stats(4, 2) },
    });
    const [item] = buildReviewQueue(base);
    const hidden = buildReviewQueue({ ...base, dismissed: { [item.id]: item.signature } });
    expect(hidden).toEqual([]);
  });

  it("resurfaces a dismissed item once its signal (signature) changes", () => {
    const base = input({
      agents: [agent({ id: "a" })],
      unseenResults: { a: true },
      gitShortstats: { a: stats(4, 2) },
    });
    const [item] = buildReviewQueue(base);
    // Dismissed at the old signature, but the diff grew → new signature, so the
    // stored mark no longer matches and the item returns.
    const grown = buildReviewQueue({
      ...base,
      gitShortstats: { a: stats(9, 2) },
      dismissed: { [item.id]: item.signature },
    });
    expect(grown).toHaveLength(1);
    expect(grown[0].id).toBe("agent:a");
  });

  // ── tests evidence (verify on turn end) ───────────────────────────────────

  it("feeds a passing/failing tests verdict onto an existing card", () => {
    const base = {
      agents: [agent({ id: "a" })],
      unseenResults: { a: true },
      gitShortstats: { a: stats(4, 2) },
    };
    const pass = buildReviewQueue(input({ ...base, verificationReports: { a: report("passed") } }));
    expect(pass).toHaveLength(1);
    expect(pass[0].tests).toBe("passed");

    for (const outcome of ["failed", "timed_out", "setup_failed"] as const) {
      const q = buildReviewQueue(input({ ...base, verificationReports: { a: report(outcome) } }));
      expect(q[0].tests).toBe("failed");
    }
  });

  it("shows no tests verdict when unknown, skipped, or absent (never a fake state)", () => {
    const base = {
      agents: [agent({ id: "a" })],
      unseenResults: { a: true },
      gitShortstats: { a: stats(4, 2) },
    };
    // No report at all.
    expect(buildReviewQueue(input(base))[0].tests).toBeUndefined();
    // A skipped test (no command resolved) is not a verdict.
    const skipped = buildReviewQueue(
      input({ ...base, verificationReports: { a: report("skipped") } }),
    );
    expect(skipped[0].tests).toBeUndefined();
  });

  it("does not create a card from a tests verdict alone", () => {
    // No unseen/diff/PR signals → no card, even with a report present.
    const q = buildReviewQueue(
      input({ agents: [agent({ id: "a" })], verificationReports: { a: report("failed") } }),
    );
    expect(q).toEqual([]);
  });

  it("resurfaces a dismissed card when a fresh tests verdict flips", () => {
    const base = input({
      agents: [agent({ id: "a" })],
      unseenResults: { a: true },
      gitShortstats: { a: stats(4, 2) },
      verificationReports: { a: report("passed") },
    });
    const [item] = buildReviewQueue(base);
    // Dismissed while green; a later failing verification changes the signature.
    const flipped = buildReviewQueue({
      ...base,
      verificationReports: { a: report("failed") },
      dismissed: { [item.id]: item.signature },
    });
    expect(flipped).toHaveLength(1);
    expect(flipped[0].tests).toBe("failed");
  });

  // ── staleness (§2) ──────────────────────────────────────────────────────────

  it("feeds staleness onto an existing card when the base has moved", () => {
    const q = buildReviewQueue(
      input({
        agents: [agent({ id: "a" })],
        unseenResults: { a: true },
        gitShortstats: { a: stats(4, 2) },
        gitMeta: { a: meta({ base: "main", behind: 3 }) },
      }),
    );
    expect(q).toHaveLength(1);
    expect(q[0].staleness).toEqual({ base: "main", behind: 3 });
  });

  it("never fakes a zero/unknown staleness, and never creates a card on its own", () => {
    // behind 0 and behind null both render nothing.
    for (const behind of [0, null]) {
      const q = buildReviewQueue(
        input({
          agents: [agent({ id: "a" })],
          unseenResults: { a: true },
          gitShortstats: { a: stats(1, 0) },
          gitMeta: { a: meta({ behind }) },
        }),
      );
      expect(q[0].staleness).toBeNull();
    }
    // Staleness alone (no other reason) does NOT surface a card.
    expect(
      buildReviewQueue(
        input({ agents: [agent({ id: "a" })], gitMeta: { a: meta({ behind: 5 }) } }),
      ),
    ).toEqual([]);
  });

  it("surfaces staleness from a secondary checkout — stalest wins, repo named", () => {
    const q = buildReviewQueue(
      input({
        agents: [agent({ id: "a" })],
        unseenResults: { a: true },
        gitShortstats: { a: stats(1, 0) },
        gitMeta: {
          a: meta({ behind: 0 }), // primary fresh
          "a::pkg/api": meta({ base: "main", behind: 4 }), // secondary stale
        },
      }),
    );
    expect(q[0].staleness).toEqual({ base: "main", behind: 4, repo: "pkg/api" });
  });

  // ── overlap hints (§4) ────────────────────────────────────────────────────────

  it("adds pairwise overlap hints for agents on the same repo touching shared files", () => {
    const q = buildReviewQueue(
      input({
        agents: [repoAgent("a", "/repo"), repoAgent("b", "/repo")],
        // Both surface for unseen results so they have cards to decorate.
        unseenResults: { a: true, b: true },
        gitShortstats: { a: stats(1, 0), b: stats(1, 0) },
        gitMeta: {
          a: meta({ files: ["src/x.ts", "src/y.ts"] }),
          b: meta({ files: ["src/y.ts", "src/z.ts"] }),
        },
      }),
    );
    const a = q.find((i) => i.id === "agent:a");
    const b = q.find((i) => i.id === "agent:b");
    expect(a?.overlaps).toEqual([{ agentName: "b", count: 1 }]);
    expect(b?.overlaps).toEqual([{ agentName: "a", count: 1 }]);
  });

  it("no overlap hint when agents are on different repos or share no files", () => {
    const q = buildReviewQueue(
      input({
        agents: [repoAgent("a", "/repo1"), repoAgent("b", "/repo2")],
        unseenResults: { a: true, b: true },
        gitShortstats: { a: stats(1, 0), b: stats(1, 0) },
        gitMeta: {
          a: meta({ files: ["src/x.ts"] }),
          b: meta({ files: ["src/x.ts"] }), // same path, different repo
        },
      }),
    );
    expect(q.find((i) => i.id === "agent:a")?.overlaps).toBeUndefined();
  });

  it("detects overlaps in secondary checkouts of the same source repo", () => {
    const twoRepo = (id: string) =>
      agent({
        id,
        repos: [
          { repo_path: `/primary-${id}`, subdir: "", parent_branch: "main" },
          { repo_path: "/shared", subdir: "pkg/api", parent_branch: "main" },
        ],
      });
    const q = buildReviewQueue(
      input({
        agents: [twoRepo("a"), twoRepo("b")],
        unseenResults: { a: true, b: true },
        gitShortstats: { a: stats(1, 0), b: stats(1, 0) },
        // Only the secondary checkouts (distinct primaries) touch shared files.
        gitMeta: {
          "a::pkg/api": meta({ files: ["src/x.ts"] }),
          "b::pkg/api": meta({ files: ["src/x.ts", "src/y.ts"] }),
        },
      }),
    );
    expect(q.find((i) => i.id === "agent:a")?.overlaps).toEqual([{ agentName: "b", count: 1 }]);
    expect(q.find((i) => i.id === "agent:b")?.overlaps).toEqual([{ agentName: "a", count: 1 }]);
  });

  // ── merge fan-out (§3) ────────────────────────────────────────────────────────

  it("raises ONE fan-out item when a sibling merges and others are behind", () => {
    const mergedPr: PrState = { ...openPr(42), state: "merged", title: "Add auth" };
    const q = buildReviewQueue(
      input({
        agents: [
          repoAgent("shipped", "/repo"),
          repoAgent("behind1", "/repo"),
          repoAgent("behind2", "/repo"),
        ],
        prStates: { shipped: mergedPr },
        gitMeta: {
          shipped: meta({ behind: 0 }),
          behind1: meta({ behind: 2 }),
          behind2: meta({ behind: 5 }),
        },
      }),
    );
    expect(q).toHaveLength(1);
    const f = q[0];
    expect(f.kind).toBe("fanout");
    expect(f.bucket).toBe(BUCKET.fanout);
    expect(f.id).toBe("fanout:/repo:main");
    expect(f.fanout?.base).toBe("main");
    expect(f.fanout?.merged).toEqual({ title: "Add auth", number: 42, url: "https://gh/pr/42" });
    // Both behind siblings are affected (sorted by name); the shipped agent is
    // excluded (its HEAD is the base, behind 0).
    expect(f.fanout?.agents.map((a) => a.agentId)).toEqual(["behind1", "behind2"]);
    expect(f.goal).toContain("2 agents are now behind main");
  });

  it("no fan-out when nobody is behind, or nobody merged", () => {
    const mergedPr: PrState = { ...openPr(1), state: "merged" };
    // Merged, but siblings caught up (behind 0) → no fan-out.
    expect(
      buildReviewQueue(
        input({
          agents: [repoAgent("shipped", "/repo"), repoAgent("sib", "/repo")],
          prStates: { shipped: mergedPr },
          gitMeta: { sib: meta({ behind: 0 }) },
        }),
      ),
    ).toEqual([]);
    // Behind, but no merged PR on the repo (a teammate push) → only per-agent
    // staleness chips, no fan-out card.
    expect(
      buildReviewQueue(
        input({
          agents: [repoAgent("a", "/repo"), repoAgent("b", "/repo")],
          gitMeta: { a: meta({ behind: 3 }), b: meta({ behind: 3 }) },
        }),
      ),
    ).toEqual([]);
  });

  it("a fan-out item is dismissible and resurfaces when the affected set changes", () => {
    const mergedPr: PrState = { ...openPr(7), state: "merged", title: "T" };
    const base = input({
      agents: [repoAgent("shipped", "/repo"), repoAgent("behind1", "/repo")],
      prStates: { shipped: mergedPr },
      gitMeta: { behind1: meta({ behind: 2 }) },
    });
    const [item] = buildReviewQueue(base);
    expect(item.kind).toBe("fanout");
    // Dismissed at its current signature → hidden.
    expect(buildReviewQueue({ ...base, dismissed: { [item.id]: item.signature } })).toEqual([]);
    // The behind count changed → new signature, dismissal expires, item returns.
    const moved = buildReviewQueue({
      ...base,
      gitMeta: { behind1: meta({ behind: 9 }) },
      dismissed: { [item.id]: item.signature },
    });
    expect(moved).toHaveLength(1);
    expect(moved[0].id).toBe(item.id);
  });

  it("fan-out excludes agents that can't act (stopped/errored)", () => {
    const mergedPr: PrState = { ...openPr(3), state: "merged" };
    const q = buildReviewQueue(
      input({
        agents: [
          repoAgent("shipped", "/repo"),
          repoAgent("stopped", "/repo", { status: "stopped" }),
          repoAgent("live", "/repo", { status: "idle" }),
        ],
        prStates: { shipped: mergedPr },
        gitMeta: {
          stopped: meta({ behind: 2 }),
          live: meta({ behind: 2 }),
        },
      }),
    );
    expect(q).toHaveLength(1);
    expect(q[0].fanout?.agents.map((a) => a.agentId)).toEqual(["live"]);
  });
});
