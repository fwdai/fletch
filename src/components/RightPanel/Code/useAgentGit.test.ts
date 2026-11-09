import { describe, expect, it } from "vitest";
import type { FileStatus, GitState } from "@/api";
import { aggregateGit, type CheckoutGit } from "./useAgentGit";

const file = (path: string): FileStatus => ({
  path,
  kind: "modified",
  staged: false,
  additions: 1,
  deletions: 0,
});

const state = (ahead: number, ...paths: string[]) =>
  ({ ahead, files: paths.map(file) }) as GitState;
const checkout = (subdir: string, extra: Omit<CheckoutGit, "subdir"> = {}): CheckoutGit => ({
  subdir,
  ...extra,
});

describe("aggregateGit", () => {
  it("is unknown, not empty, until every checkout has answered", () => {
    expect(aggregateGit([checkout("app")])).toMatchObject({ answered: false, files: null });
    const half = aggregateGit([checkout("web", { state: state(0, "a.ts") }), checkout("api")]);
    expect(half).toMatchObject({ answered: false, files: null });
  });

  it("is the primary checkout's list as is for a single-repo agent", () => {
    const s = state(2, "src/a.ts", "README.md");
    const git = aggregateGit([checkout("app", { state: s })]);
    expect(git.files).toBe(s.files);
    expect(git).toMatchObject({ answered: true, ahead: 2, blocked: [] });
  });

  it("prefixes paths and sums commits ahead across a multi-repo agent", () => {
    const git = aggregateGit([
      checkout("web", { state: state(0) }),
      checkout("api", { state: state(3, "src/b.rs") }),
    ]);
    expect(git.files?.map((f) => f.path)).toEqual(["api/src/b.rs"]);
    // Only the secondary has work: the agent still has changes.
    expect(git.ahead).toBe(3);
  });

  it("counts a refused checkout as answered but names it as blocked", () => {
    const git = aggregateGit([
      checkout("web", { state: state(0, "a.ts") }),
      checkout("api", { blocked: ["core.fsmonitor"] }),
    ]);
    expect(git).toMatchObject({ answered: true, blocked: ["api"] });
    expect(git.files?.map((f) => f.path)).toEqual(["web/a.ts"]);
  });
});
