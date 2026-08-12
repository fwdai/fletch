import { describe, expect, it } from "vitest";
import type { ItemStatus, RoadmapItem } from "@/api";
import { isOnBoard, isOrderable, isRejected, isShipped, rejectedRows } from "./partition";

function item(over: Partial<RoadmapItem> & { id: string }): RoadmapItem {
  return {
    project_id: "p1",
    code: over.id.toUpperCase(),
    title: "t",
    why: "",
    horizon: "next",
    status: "open",
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
    close_reason: null,
    issue_url: null,
    created_at: 0,
    updated_at: 0,
    ...over,
  };
}

const at = (status: ItemStatus) => item({ id: status, status });

describe("the status partition", () => {
  it("keeps a rejected item off the horizon groups without calling it shipped", () => {
    // The regression this file exists for: `status !== "done"` used to be the
    // whole answer, which would have drawn rejected rows in the groups and
    // inflated their counts.
    const rejected = item({ id: "x", status: "rejected", close_reason: "duplicate" });
    expect(isOnBoard(rejected)).toBe(false);
    expect(isShipped(rejected)).toBe(false);
    expect(isRejected(rejected)).toBe(true);
  });

  it("boards every working status and only counts done as shipped", () => {
    for (const status of ["proposed", "open", "queued", "active", "in_review"] as const) {
      expect(isOnBoard(at(status))).toBe(true);
      expect(isShipped(at(status))).toBe(false);
    }
    expect(isOnBoard(at("done"))).toBe(false);
    expect(isShipped(at("done"))).toBe(true);
  });

  it("never lets a rejected row into the orderable set", () => {
    // The drag's domain and the PM order ask's reference — the backend's
    // `order::is_orderable` excludes rejected too, and the two must agree.
    expect(isOrderable(at("rejected"))).toBe(false);
    expect(isOrderable(at("open"))).toBe(true);
  });
});

describe("rejectedRows", () => {
  it("lists only rejected rows, newest ruling first", () => {
    const older = item({ id: "a", status: "rejected", close_reason: "r", updated_at: 10 });
    const newer = item({ id: "b", status: "rejected", close_reason: "r", updated_at: 20 });
    const open = item({ id: "c", updated_at: 30 });
    expect(rejectedRows([older, open, newer]).map((r) => r.id)).toEqual(["b", "a"]);
  });
});
