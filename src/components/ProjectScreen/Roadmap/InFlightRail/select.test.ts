import { describe, expect, it } from "vitest";
import type { ItemStatus, PrChecks, RoadmapItem, RoadmapItemReview, WfRun } from "@/api";
import { buildInFlight } from "./select";

// ── fixtures ──────────────────────────────────────────────────────────────────

function item(over: Partial<RoadmapItem> & { id: string }): RoadmapItem {
  return {
    project_id: "p1",
    code: `MCA-${over.id}`,
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
        building: true,
        runId: "r",
        startedAt: 5_000,
      },
    ]);
  });

  it("names a pause in the same words the item card's chip uses", () => {
    const rail = railOf(item({ id: "a", run_id: "r" }), {
      runsById: new Map([["r", run({ id: "r", status: "paused", paused_reason: "approval" })]]),
    });
    expect(rail[0]).toMatchObject({
      state: "paused — needs approval",
      tone: "warn",
      building: false,
    });
  });

  it("calls a pause with no recorded reason a pause, not motion", () => {
    // The reason is what names the pause; without one there is still nothing
    // happening, and the old guard let this row through as "running".
    const rail = railOf(item({ id: "a", run_id: "r" }), {
      runsById: new Map([["r", run({ id: "r", status: "paused", paused_reason: null })]]),
    });
    expect(rail[0]).toMatchObject({ state: "paused", tone: "warn", building: false });
    // Keeps its clock: that elapsed time is what the pause is costing.
    expect(rail[0].startedAt).toBe(1_000);
  });

  it("says a run failed rather than showing it as still running", () => {
    // `runsById` carries terminal rows (`wf_list_runs` filters nothing) and the
    // drainer settles the item a tick later — or never, if that write fails.
    const rail = railOf(item({ id: "a", run_id: "r" }), {
      runsById: new Map([["r", run({ id: "r", status: "failed" })]]),
    });
    expect(rail[0]).toMatchObject({ state: "run failed", tone: "attention", building: false });
    expect(rail[0].startedAt).toBeUndefined();
  });

  it("says a run was canceled rather than showing it as still running", () => {
    const rail = railOf(item({ id: "a", run_id: "r" }), {
      runsById: new Map([["r", run({ id: "r", status: "canceled" })]]),
    });
    expect(rail[0]).toMatchObject({ state: "run canceled", tone: "warn", building: false });
    expect(rail[0].startedAt).toBeUndefined();
  });

  it("reads an ended run as finishing, with no clock left to run", () => {
    const rail = railOf(item({ id: "a", run_id: "r" }), {
      runsById: new Map([["r", run({ id: "r", status: "done", created_at: 5_000 })]]),
    });
    expect(rail[0]).toMatchObject({ state: "finishing", tone: "info", building: false });
    // A span that keeps counting on a run that ended is time nothing is spending.
    expect(rail[0].startedAt).toBeUndefined();
  });

  it("reads a run that hasn't begun as starting, and still counts it as motion", () => {
    const rail = railOf(item({ id: "a", run_id: "r" }), {
      runsById: new Map([["r", run({ id: "r", status: "pending" })]]),
    });
    expect(rail[0]).toMatchObject({ state: "starting", tone: "info", building: true });
    expect(rail[0].startedAt).toBe(1_000);
  });

  it("lets a hold outrank a live run — a held row is not being built", () => {
    // The Needs-you strip above carries the release; the rail only has to stop
    // claiming the row is moving.
    const rail = railOf(item({ id: "a", run_id: "r", hold_reason: "waiting on me" }), {
      runsById: new Map([["r", run({ id: "r", status: "running" })]]),
    });
    expect(rail[0]).toMatchObject({ state: "held", tone: "attention", building: false });
  });

  it("still lists a row the drainer claimed before its run existed", () => {
    // The queue flips the status a beat before the run row lands, and a row with
    // no run resolved is exactly the one worth seeing on the rail.
    const rail = railOf(item({ id: "a", run_id: null }));
    expect(rail[0]).toMatchObject({ state: "running", runId: undefined, startedAt: undefined });
  });

  it("falls back to plain 'running' when the run id resolves to nothing", () => {
    // `runsById` is already project-scoped; a miss must read as "no run state",
    // never as another board's.
    const rail = railOf(item({ id: "a", run_id: "elsewhere" }));
    expect(rail[0]).toMatchObject({ state: "running", tone: "info", building: true });
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
    expect(rail[0]).toMatchObject({ state: "in review", tone: "info", building: false });
  });

  it("says a PR with no number can't be watched, the way its card does", () => {
    // A URL with no number is what `merge_sweep::pollable` skips, so no gate ever
    // arrives: "in review" here would be a wait that never ends.
    const rail = railOf(
      item({
        id: "a",
        status: "in_review",
        pr_url: "https://github.com/o/r/pull/5",
        pr_number: null,
      }),
    );
    expect(rail[0]).toMatchObject({ state: "can't watch this PR", tone: "warn" });
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
