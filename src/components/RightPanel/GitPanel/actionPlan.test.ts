import { describe, expect, it } from "vitest";
import type { PrChecks, PrComment } from "@/api";
import { type PlanFacts, playbookFor } from "./actionPlan";

function checks(over: Partial<PrChecks> = {}): PrChecks {
  return {
    merge_state: "clean",
    rollup: "failing",
    total: 3,
    passed: 1,
    failed: 2,
    pending: 0,
    required_failing: [],
    runs: [],
    ...over,
  };
}
function thread(we_replied_last: boolean): PrComment {
  return {
    url: "https://x/1",
    author: "greptile",
    is_bot: true,
    body: "b",
    replies: 0,
    we_replied_last,
  } as PrComment;
}

const facts: PlanFacts = {
  base: "main",
  checks: checks({ required_failing: ["lint", "test"] }),
  comments: { unresolved: [thread(false), thread(false), thread(true)] },
};

describe("playbookFor", () => {
  it("maps each delegated key to its playbook, naming the base where it matters", () => {
    expect(playbookFor("agent-commit", facts)).toEqual({ kind: "commit", action: "commit" });
    expect(playbookFor("agent-commit-push", facts)).toEqual({
      kind: "commit-push",
      action: "commit-push",
    });
    expect(playbookFor("agent-commit-pr", facts)).toEqual({
      kind: "commit-pr",
      action: "commit-pr",
      params: { base: "main" },
    });
    expect(playbookFor("agent-open-pr", facts)).toEqual({
      kind: "open-pr",
      action: "open-pr",
      params: { base: "main" },
    });
    expect(playbookFor("agent-update-branch", facts)).toEqual({
      kind: "update-branch",
      action: "update-branch",
      params: { base: "main" },
    });
    expect(playbookFor("push", facts)).toEqual({ kind: "push", action: "push" });
  });

  it("names resolve's playbook resolve-conflicts", () => {
    expect(playbookFor("agent-resolve", facts)).toEqual({
      kind: "resolve",
      action: "resolve-conflicts",
    });
  });

  it("lists the required failing checks for fix-checks", () => {
    expect(playbookFor("agent-fix", facts)?.params).toEqual({ failing: "lint, test" });
    expect(playbookFor("agent-fix", { ...facts, checks: null })?.params).toEqual({ failing: "" });
  });

  it("counts only the threads waiting on us, not the ones we pushed back on", () => {
    expect(playbookFor("agent-resolve-comments", facts)).toEqual({
      kind: "resolve-comments",
      action: "resolve-comments",
      params: { count: "2" },
    });
    expect(playbookFor("agent-resolve-comments", { ...facts, comments: null })?.params).toEqual({
      count: "0",
    });
  });

  it("has no plan for a key the agent doesn't run", () => {
    for (const key of ["open-pr", "commit-pr-direct", "merge", "pull", "loading", "nope"]) {
      expect(playbookFor(key, facts)).toBeNull();
    }
  });
});
