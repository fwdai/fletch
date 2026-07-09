import { describe, expect, it } from "vitest";
import type { TrackedRepo } from "@/api";
import { prSnapshot } from "@/util/prState";

const repo = (overrides: Partial<TrackedRepo> = {}): TrackedRepo => ({
  repo_path: "/r",
  subdir: "repo",
  branch: "feat/x",
  parent_branch: "main",
  pr_number: 42,
  pr_url: "https://github.com/o/r/pull/42",
  pr_title: "feat: x",
  pr_state: "merged",
  ...overrides,
});

describe("prSnapshot", () => {
  it("rebuilds the persisted PR state from the repo record", () => {
    expect(prSnapshot(repo())).toEqual({
      number: 42,
      url: "https://github.com/o/r/pull/42",
      state: "merged",
      title: "feat: x",
      mergeable: false,
    });
  });

  it("is null without a bound PR number or persisted state", () => {
    expect(prSnapshot(undefined)).toBeNull();
    expect(prSnapshot(repo({ pr_number: null }))).toBeNull();
    expect(prSnapshot(repo({ pr_state: null }))).toBeNull();
  });

  it("rejects an unknown state string rather than fabricating a badge", () => {
    expect(prSnapshot(repo({ pr_state: "weird" }))).toBeNull();
  });

  it("degrades missing url/title to empty strings", () => {
    const pr = prSnapshot(repo({ pr_url: null, pr_title: null }));
    expect(pr).toMatchObject({ number: 42, url: "", title: "" });
  });
});
