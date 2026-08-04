import { describe, expect, it } from "vitest";
import type { PrComment, RoadmapItem } from "@/api";
import { reviewFeedbackPrompt } from "./reviewPrompt";

function item(over: Partial<RoadmapItem> = {}): RoadmapItem {
  return {
    id: "i1",
    project_id: "p1",
    code: "FLT-142",
    title: "Say what the board is waiting on",
    why: "the why nobody should be re-litigating here",
    horizon: "now",
    status: "in_review",
    rank: 1,
    area: null,
    source: "pm",
    accept: ["a criterion the fix agent must not treat as its job"],
    deps: [],
    agent_id: null,
    workflow_def_id: null,
    run_id: "run-1",
    pr_url: "https://github.com/o/r/pull/598",
    pr_number: 598,
    created_at: 0,
    hold_reason: null,
    held_by: null,
    held_at: null,
    updated_at: 0,
    ...over,
  };
}

function thread(over: Partial<PrComment> = {}): PrComment {
  return {
    id: "t1",
    author: "greptile-apps",
    is_bot: true,
    body: "This can be null when the row is gone.",
    path: "src/foo.ts",
    line: 42,
    url: "https://github.com/o/r/pull/598#discussion_r1",
    replies: 0,
    we_replied_last: false,
    ...over,
  };
}

describe("reviewFeedbackPrompt", () => {
  it("has no prompt when nothing is unresolved — which is what gates the action", () => {
    expect(reviewFeedbackPrompt(item(), [])).toBeNull();
  });

  it("names the item and the PR, quotes every thread, and ends with the instruction", () => {
    const prompt = reviewFeedbackPrompt(item(), [
      thread(),
      thread({
        id: "t2",
        author: "alex",
        is_bot: false,
        body: "Rename this.",
        path: "b.rs",
        line: 7,
      }),
    ]);
    expect(prompt).toContain("FLT-142: Say what the board is waiting on");
    expect(prompt).toContain("PR #598 has 2 unresolved review threads:");
    expect(prompt).toContain("1. @greptile-apps — src/foo.ts:42");
    expect(prompt).toContain("This can be null when the row is gone.");
    expect(prompt).toContain("2. @alex — b.rs:7");
    expect(prompt).toContain("Rename this.");
    expect(prompt).toContain("Address each thread on this PR's branch; push when green.");
    // The link, so the agent can read the exchange rather than only our summary.
    expect(prompt).toContain("https://github.com/o/r/pull/598");
  });

  it("leaves the item's why and acceptance criteria out — the build already happened", () => {
    const prompt = reviewFeedbackPrompt(item(), [thread()]);
    expect(prompt).not.toContain("re-litigating");
    expect(prompt).not.toContain("must not treat as its job");
  });

  it("counts one thread in the singular", () => {
    expect(reviewFeedbackPrompt(item(), [thread()])).toContain("1 unresolved review thread:");
  });

  it("says so when a thread has no file to anchor to", () => {
    const prompt = reviewFeedbackPrompt(item(), [thread({ path: null, line: null })]);
    expect(prompt).toContain("1. @greptile-apps — no file — the line it was on is gone");
  });

  it("keeps a line number off a thread that has a path but no line", () => {
    const prompt = reviewFeedbackPrompt(item(), [thread({ line: null })]);
    expect(prompt).toContain("1. @greptile-apps — src/foo.ts\n");
  });

  it("flags a thread we answered last, so the agent doesn't re-argue it", () => {
    const prompt = reviewFeedbackPrompt(item(), [thread({ we_replied_last: true })]);
    expect(prompt).toContain("we answered this one last");
  });

  it("indents a multi-line comment body under its thread", () => {
    const prompt = reviewFeedbackPrompt(item(), [thread({ body: "First line.\n\nSecond line." })]);
    expect(prompt).toContain("   First line.\n\n   Second line.");
  });

  it("falls back to a nameless PR when the item somehow has no number", () => {
    const prompt = reviewFeedbackPrompt(item({ pr_number: null, pr_url: null }), [thread()]);
    expect(prompt).toContain("its pull request has 1 unresolved review thread:");
  });
});
