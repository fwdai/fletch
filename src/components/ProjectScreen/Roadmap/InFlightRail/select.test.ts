import { describe, expect, it } from "vitest";
import type { ItemStatus, PrChecks, RoadmapItem, RoadmapItemReview, WfRun } from "@/api";
import { buildInFlight } from "./select";

// ── fixtures ──────────────────────────────────────────────────────────────────

function item(over: Partial<RoadmapItem> & { id: string }): RoadmapItem {
  return {
    project_id: "p1",
    code: `MCA-${over.id}`,
    parent_id: null,
    title: `Item ${over.id}`,
    why: "",
    horizon: "now",
    status: "active",
    rank: 1,
    area: null,
    source: "user",
    accept: [],
    deps: [],
    agent_id: null,
    workflow_def_id: null,
    run_id: null,
    pr_url: null,
    pr_number: null,
    hold_reason: null,
    held_by: null,
    held_at: null,
    created_at: 0,
    updated_at: 0,
    ...over,
  };
}

function run(over: Partial<WfRun> & { id: string }): WfRun {
  return {
    definition_id: null,
    parent_run_id: null,
    name: `run-${over.id}`,
    spec: null,
    task: "",
    project_id: "p1",
    repo_path: "/repo",
    run_dir: "/runs/x",
    branch: "wf/x",
    base_sha: "abc",
    status: "running",
    paused_reason: null,
    cursor: null,
    budgets: null,
    spent: null,
    error: null,
    pr_number: null,
    pr_url: null,
    roadmap_item_id: null,
    created_at: 1_000,
    updated_at: 2_000,
    ...over,
  };
}

function checks(over: Partial<PrChecks> = {}): PrChecks {
  return {
    merge_state: "clean",
    rollup: "passing",
    total: 1,
    passed: 1,
    failed: 0,
    pending: 0,
    required_failing: [],
    runs: [],
    ...over,
  };
}

function review(over: Partial<RoadmapItemReview> = {}): RoadmapItemReview {
  return { checks: checks(), comments: null, head_ref: null, base_ref: null, ...over };
}

type Input = Parameters<typeof buildInFlight>[0];

function input(over: Partial<Input> = {}): Input {
  return {
    items: [],
    runsById: new Map<string, WfRun>(),
    reviews: new Map<string, RoadmapItemReview>(),
    ...over,
  };
}

/** The one-item board every case below is a variation of. */
function railOf(row: RoadmapItem, over: Partial<Input> = {}) {
  return buildInFlight(input({ items: [row], ...over }));
}

// ── what is being built ───────────────────────────────────────────────────────

describe("buildInFlight — active rows", () => {
  it("reports a running run, with the run and its start time for the clock", () => {
    const rail = railOf(item({ id: "a", run_id: "r", title: "Ship the drainer" }), {
      runsById: new Map([["r", run({ id: "r", status: "running", created_at: 5_000 })]]),
    });
    expect(rail).toEqual([
      {
        id: "a",
        kind: "active",
        code: "MCA-a",
        title: "Ship the drainer",
        state: "running",
        tone: "info",
        runId: "r",
        startedAt: 5_000,
      },
    ]);
  });

  it("names a pause in the same words the item card's chip uses", () => {
    const rail = railOf(item({ id: "a", run_id: "r" }), {
      runsById: new Map([["r", run({ id: "r", status: "paused", paused_reason: "approval" })]]),
    });
    expect(rail[0]).toMatchObject({ state: "paused — needs approval", tone: "warn" });
  });

  it("still lists a row the drainer claimed before its run existed", () => {
    // The queue flips the status a beat before the run row lands, and a row with
    // no run resolved is exactly the one worth seeing on the rail.
    const rail = railOf(item({ id: "a", run_id: null }));
    expect(rail[0]).toMatchObject({ state: "running", runId: undefined, startedAt: undefined });
  });

  it("says nothing about a run this project doesn't own", () => {
    // `runsById` is already project-scoped; a miss must read as "no run state",
    // never as another board's.
    const rail = railOf(item({ id: "a", run_id: "elsewhere" }));
    expect(rail[0]).toMatchObject({ state: "running", tone: "info" });
  });
});

// ── what is waiting to ship ───────────────────────────────────────────────────

describe("buildInFlight — in-review rows", () => {
  const gates: { merge_state: PrChecks["merge_state"]; state: string; tone: string }[] = [
    { merge_state: "clean", state: "ready to merge", tone: "ready" },
    { merge_state: "unstable", state: "optional checks failing", tone: "warn" },
    { merge_state: "dirty", state: "conflicts with main", tone: "attention" },
    { merge_state: "behind", state: "behind main", tone: "attention" },
    { merge_state: "draft", state: "draft", tone: "info" },
    { merge_state: "unknown", state: "checking…", tone: "info" },
  ];

  for (const { merge_state, state, tone } of gates) {
    it(`${merge_state} → "${state}"`, () => {
      const rail = railOf(item({ id: "a", status: "in_review", pr_number: 7 }), {
        reviews: new Map([["a", review({ checks: checks({ merge_state }), base_ref: "main" })]]),
      });
      expect(rail[0]).toMatchObject({ kind: "in_review", state, tone });
    });
  }

  it("splits a blocked gate the way the shared classifier does", () => {
    // Failing *required* checks are agent-fixable; a pure review gate is not.
    const failing = railOf(item({ id: "a", status: "in_review" }), {
      reviews: new Map([
        ["a", review({ checks: checks({ merge_state: "blocked", required_failing: ["test"] }) })],
      ]),
    });
    expect(failing[0]).toMatchObject({ state: "checks failing", tone: "attention" });

    const gated = railOf(item({ id: "a", status: "in_review" }), {
      reviews: new Map([["a", review({ checks: checks({ merge_state: "blocked", failed: 3 }) })]]),
    });
    expect(gated[0]).toMatchObject({ state: "review required", tone: "attention" });
  });

  it("reads a missing answer as plain 'in review', never as a clean gate", () => {
    const rail = railOf(item({ id: "a", status: "in_review", pr_number: 7 }));
    expect(rail[0]).toMatchObject({ state: "in review", tone: "info" });
  });

  it("carries no run or clock — a PR's age is not this board's", () => {
    const rail = railOf(item({ id: "a", status: "in_review", run_id: "r" }), {
      runsById: new Map([["r", run({ id: "r" })]]),
    });
    expect(rail[0]).not.toHaveProperty("runId");
    expect(rail[0]).not.toHaveProperty("startedAt");
  });
});

// ── what the rail is not about ────────────────────────────────────────────────

describe("buildInFlight — scope", () => {
  it("ignores every status that isn't in motion", () => {
    const resting: ItemStatus[] = ["proposed", "open", "queued", "done"];
    expect(
      buildInFlight(input({ items: resting.map((status, i) => item({ id: `${i}`, status })) })),
    ).toEqual([]);
  });

  it("renders nothing for an empty board", () => {
    expect(buildInFlight(input())).toEqual([]);
  });

  it("puts what is building above what is shipping, each in board order", () => {
    const rail = buildInFlight(
      input({
        items: [
          item({ id: "r2", status: "in_review", rank: 40 }),
          item({ id: "a2", status: "active", rank: 30 }),
          item({ id: "r1", status: "in_review", rank: 20 }),
          item({ id: "a1", status: "active", rank: 10 }),
        ],
      }),
    );
    expect(rail.map((e) => e.id)).toEqual(["a1", "a2", "r1", "r2"]);
  });

  it("breaks a rank tie on the code, so the order never flickers", () => {
    const rail = buildInFlight(
      input({
        items: [
          item({ id: "b", code: "MCA-9", rank: 10 }),
          item({ id: "a", code: "MCA-2", rank: 10 }),
        ],
      }),
    );
    expect(rail.map((e) => e.code)).toEqual(["MCA-2", "MCA-9"]);
  });
});
