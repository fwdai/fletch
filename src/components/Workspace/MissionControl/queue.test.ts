import { describe, expect, it } from "vitest";
import type { AgentRecord, PrChecks, PrComments, PrState, ShortStats, WfRun } from "@/api";
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
  mergeable: true,
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

function input(over: Partial<QueueInput>): QueueInput {
  return {
    agents: [],
    gitShortstats: {},
    unseenResults: {},
    prStates: {},
    prChecks: {},
    prComments: {},
    runs: [],
    dismissed: {},
    ...over,
  };
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
    // The chips show the PR carrying the issue — the failing secondary.
    expect(q[0].pr).toEqual({ number: 2, url: "https://gh/pr/2" });
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
});
