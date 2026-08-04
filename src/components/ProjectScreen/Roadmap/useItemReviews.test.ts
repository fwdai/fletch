import { describe, expect, it } from "vitest";
import type { PrChecks, PrComment, RoadmapItem, RoadmapItemReview } from "@/api";
import { reviewGate, reviewTargets } from "./useItemReviews";

function item(over: Partial<RoadmapItem> = {}): RoadmapItem {
  return {
    id: "i1",
    project_id: "p1",
    code: "FLT-1",
    parent_id: null,
    title: "t",
    why: "",
    horizon: "now",
    status: "in_review",
    rank: 1,
    area: null,
    source: "user",
    accept: [],
    deps: [],
    agent_id: null,
    workflow_def_id: null,
    run_id: null,
    pr_url: null,
    pr_number: 1,
    created_at: 0,
    hold_reason: null,
    held_by: null,
    held_at: null,
    updated_at: 0,
    ...over,
  };
}

describe("reviewTargets", () => {
  it("polls only in-review items that carry a PR number", () => {
    const rows = [
      item({ id: "review", status: "in_review", pr_number: 5 }),
      item({ id: "active", status: "active", pr_number: 6 }),
      item({ id: "queued", status: "queued", pr_number: 7 }),
      item({ id: "open", status: "open", pr_number: 8 }),
      item({ id: "done", status: "done", pr_number: 9 }),
      item({ id: "proposed", status: "proposed", pr_number: 10 }),
    ];
    expect(reviewTargets(rows)).toEqual(["review"]);
  });

  it("skips an in-review item with a URL but no number — there is nothing to ask with", () => {
    const rows = [
      item({ id: "unpollable", pr_number: null, pr_url: "https://github.com/o/r/pull/5" }),
    ];
    expect(reviewTargets(rows)).toEqual([]);
  });

  it("polls nothing on a board with nothing in review", () => {
    expect(reviewTargets([item({ status: "open" })])).toEqual([]);
    expect(reviewTargets([])).toEqual([]);
  });

  it("is order-stable, so board churn doesn't re-arm the poll", () => {
    const a = item({ id: "b-id", pr_number: 1 });
    const b = item({ id: "a-id", pr_number: 2 });
    expect(reviewTargets([a, b])).toEqual(["a-id", "b-id"]);
    expect(reviewTargets([b, a])).toEqual(["a-id", "b-id"]);
  });
});

function checks(over: Partial<PrChecks> = {}): PrChecks {
  return {
    merge_state: "clean",
    rollup: "passing",
    total: 3,
    passed: 3,
    failed: 0,
    pending: 0,
    required_failing: [],
    runs: [],
    ...over,
  };
}

function thread(over: Partial<PrComment> = {}): PrComment {
  return {
    id: "t1",
    author: "a",
    is_bot: false,
    body: "b",
    path: null,
    line: null,
    url: "u",
    replies: 0,
    we_replied_last: false,
    ...over,
  };
}

function review(over: Partial<RoadmapItemReview> = {}): RoadmapItemReview {
  return { checks: checks(), comments: null, head_ref: null, base_ref: null, ...over };
}

describe("reviewGate", () => {
  it("takes its verdict and its words from the shared merge gate", () => {
    const gate = reviewGate(review({ checks: checks({ merge_state: "clean" }) }));
    expect(gate).toMatchObject({
      situation: "ready",
      tone: "ready",
      mergeAllowed: true,
      label: "ready to merge",
    });
  });

  it("counts *required* failing checks, not the raw failure count", () => {
    // The gate's `checksFailed` is what splits `blocked` into agent-fixable vs a
    // pure review gate — and `required_failing` is what that means. Reading
    // `checks.failed` instead is the drift mergeGate.ts exists to prevent: here
    // it would call a review gate "checks failing".
    const gate = reviewGate(
      review({
        checks: checks({ merge_state: "blocked", failed: 4, required_failing: [] }),
      }),
    );
    expect(gate.situation).toBe("review-required");
    expect(gate.failing).toBe(0);

    const failing = reviewGate(
      review({
        checks: checks({ merge_state: "blocked", failed: 0, required_failing: ["build", "test"] }),
      }),
    );
    expect(failing.situation).toBe("checks-failing");
    expect(failing.failing).toBe(2);
    expect(failing.mergeAllowed).toBe(false);
  });

  it("names the PR's base branch when the read told us one", () => {
    const known = reviewGate(
      review({ checks: checks({ merge_state: "dirty" }), base_ref: "main" }),
    );
    expect(known.label).toBe("conflicts with main");
    const unknown = reviewGate(review({ checks: checks({ merge_state: "dirty" }) }));
    expect(unknown.label).toBe("conflicts with base");
  });

  it("reads a missing checks answer as still computing, never as merge-ready", () => {
    // The board has no `PrState` to fall back on (the sweep owns "did it merge"),
    // so a degraded read must render as computing — not as a false all-clear and
    // not as a false conflict.
    const gate = reviewGate(review({ checks: null }));
    expect(gate.situation).toBe("computing");
    expect(gate.mergeAllowed).toBe(false);
    expect(gate.failing).toBe(0);
  });

  it("counts unresolved threads, whoever is waiting on whom", () => {
    expect(reviewGate(review()).threads).toBe(0);
    expect(reviewGate(review({ comments: { unresolved: [] } })).threads).toBe(0);
    expect(
      reviewGate(
        review({
          comments: { unresolved: [thread(), thread({ id: "t2", we_replied_last: true })] },
        }),
      ).threads,
    ).toBe(2);
  });
});
