// The Git tab's action table in src/screens/Agent/GitTab/actions.ts.

import type { GitState } from "@desktop/api/types/git";
import type { CheckRun, PrChecks, PrComment, PrComments, PrState } from "@desktop/api/types/pr";
import { describe, expect, it } from "vitest";
import {
  checkOutcome,
  type GitActionsInput,
  gitActionsFor,
  isCommitAction,
} from "../src/screens/Agent/GitTab/actions";

const git = (over: Partial<GitState> = {}): GitState => ({
  branch: "feat/x",
  parent_branch: "main",
  ahead: 0,
  behind: 0,
  unpushed: 0,
  files: [],
  additions: 0,
  deletions: 0,
  has_origin: true,
  ...over,
});

const file = (kind: GitState["files"][number]["kind"] = "modified") => ({
  path: "a.ts",
  kind,
  staged: false,
  additions: 1,
  deletions: 0,
});

const pr = (over: Partial<PrState> = {}): PrState => ({
  number: 7,
  url: "https://github.com/o/r/pull/7",
  state: "open",
  title: "x",
  mergeable: "mergeable",
  ...over,
});

const run = (over: Partial<CheckRun> = {}): CheckRun => ({
  name: "build",
  status: "completed",
  conclusion: "success",
  required: true,
  url: null,
  started_at: null,
  completed_at: null,
  ...over,
});

const checks = (over: Partial<PrChecks> = {}): PrChecks => ({
  merge_state: "clean",
  rollup: "passing",
  total: 1,
  passed: 1,
  failed: 0,
  pending: 0,
  required_failing: [],
  runs: [run()],
  ...over,
});

const thread = (over: Partial<PrComment> = {}): PrComment => ({
  id: "t1",
  author: "reviewer",
  is_bot: false,
  body: "nit",
  path: null,
  line: null,
  url: "https://github.com/o/r/pull/7#discussion_r1",
  replies: 0,
  we_replied_last: false,
  ...over,
});

const threads = (...items: PrComment[]): PrComments => ({ unresolved: items });

const input = (over: Partial<GitActionsInput> = {}): GitActionsInput => ({
  git: git(),
  pr: null,
  checks: null,
  threads: null,
  canMerge: true,
  base: "main",
  ...over,
});

const keys = (over: Partial<GitActionsInput> = {}) => gitActionsFor(input(over)).map((a) => a.key);

describe("gitActionsFor", () => {
  it("offers nothing for a clean tree with no PR", () => {
    expect(keys()).toEqual([]);
  });

  it("picks the commit / push playbook the Changes footer always did", () => {
    expect(gitActionsFor(input({ git: git({ files: [file()] }) }))).toEqual([
      {
        key: "commit-pr",
        kind: "delegate",
        playbook: "commit-pr",
        label: "Commit & open PR with agent",
        params: { base: "main" },
      },
    ]);
    expect(gitActionsFor(input({ git: git({ unpushed: 2 }) }))[0]).toMatchObject({
      key: "open-pr",
      params: { base: "main" },
    });
    expect(gitActionsFor(input({ git: git({ files: [file()] }), pr: pr() }))[0]).toMatchObject({
      key: "commit-push",
      label: "Commit & push to #7 with agent",
    });
    expect(gitActionsFor(input({ git: git({ unpushed: 1 }), pr: pr() }))[0]).toMatchObject({
      key: "push",
      label: "Push to #7 with agent",
    });
  });

  it("a merged PR no longer takes pushes — new work opens a new one", () => {
    expect(keys({ git: git({ files: [file()] }), pr: pr({ state: "merged" }) })).toEqual([
      "commit-pr",
    ]);
  });

  it("resolving local conflicts is the only action a conflicted tree gets", () => {
    const list = gitActionsFor(input({ git: git({ files: [file("conflicted"), file()] }) }));
    expect(list.map((a) => a.key)).toEqual(["resolve-conflicts"]);
    expect(list[0]).toMatchObject({ kind: "delegate", playbook: "resolve-conflicts" });
    expect(list[0].params).toBeUndefined();
  });

  it("merges only when the gate is open and the host can", () => {
    expect(gitActionsFor(input({ pr: pr(), checks: checks() }))).toEqual([
      { key: "merge", kind: "merge", label: "Merge PR #7" },
    ]);
    expect(keys({ pr: pr(), checks: checks(), canMerge: false })).toEqual([]);
    // No checks data: `mergeable` alone is never an all-clear.
    expect(keys({ pr: pr() })).toEqual([]);
    expect(keys({ pr: pr(), checks: checks({ merge_state: "blocked" }) })).toEqual([]);
  });

  it("updates the branch when it is behind or conflicting with the base", () => {
    expect(gitActionsFor(input({ pr: pr(), checks: checks({ merge_state: "behind" }) }))).toEqual([
      {
        key: "update-branch",
        kind: "delegate",
        playbook: "update-branch",
        label: "Update branch with agent",
        params: { base: "main" },
      },
    ]);
    expect(keys({ pr: pr({ mergeable: "conflicting" }) })).toEqual(["update-branch"]);
  });

  it("fixes failing checks, naming the required ones or else the red runs", () => {
    const failing = checks({
      merge_state: "blocked",
      failed: 2,
      required_failing: ["lint", "test"],
    });
    expect(gitActionsFor(input({ pr: pr(), checks: failing }))).toEqual([
      {
        key: "fix-checks",
        kind: "delegate",
        playbook: "fix-checks",
        label: "Fix failing checks with agent",
        params: { failing: "lint, test" },
      },
    ]);
    const unstable = checks({
      merge_state: "unstable",
      failed: 1,
      runs: [run(), run({ name: "e2e", conclusion: "failure", required: false })],
    });
    const list = gitActionsFor(input({ pr: pr(), checks: unstable }));
    // GitHub would still take the merge, but the red check leads.
    expect(list.map((a) => a.key)).toEqual(["fix-checks", "merge"]);
    expect(list[0].params).toEqual({ failing: "e2e" });
  });

  it("resolves the review threads awaiting us, not the ones we pushed back on", () => {
    const two = threads(
      thread(),
      thread({ id: "t2" }),
      thread({ id: "t3", we_replied_last: true }),
    );
    const list = gitActionsFor(input({ pr: pr(), checks: checks(), threads: two }));
    expect(list.map((a) => a.key)).toEqual(["resolve-comments", "merge"]);
    expect(list[0]).toMatchObject({
      label: "Resolve 2 review comments with agent",
      params: { count: "2" },
    });
    expect(
      gitActionsFor(input({ pr: pr(), checks: checks(), threads: threads(thread()) }))[0].label,
    ).toBe("Resolve 1 review comment with agent");
    expect(
      keys({ pr: pr(), checks: checks(), threads: threads(thread({ we_replied_last: true })) }),
    ).toEqual(["merge"]);
  });

  it("offers only resolve-conflicts while the tree is conflicted", () => {
    // Never a commit (it would capture the markers) and never a merge, however
    // green the PR is: the desktop's conflict state makes the same call.
    expect(
      keys({
        git: git({ files: [file("conflicted"), file("modified")], unpushed: 2 }),
        pr: pr(),
        checks: checks({ merge_state: "clean" }),
        threads: threads(thread()),
      }),
    ).toEqual(["resolve-conflicts"]);
  });

  it("orders everything most pressing first, merge last", () => {
    expect(
      keys({
        git: git({ files: [file("modified")] }),
        pr: pr(),
        checks: checks({ merge_state: "unstable", failed: 1, required_failing: ["test"] }),
        threads: threads(thread()),
      }),
    ).toEqual(["commit-push", "fix-checks", "resolve-comments", "merge"]);
  });

  it("offers nothing for a closed PR beyond the local work", () => {
    expect(
      keys({ pr: pr({ state: "closed" }), checks: checks({ merge_state: "behind" }) }),
    ).toEqual([]);
  });
});

describe("isCommitAction", () => {
  it("names the family the Changes footer shows", () => {
    const all = gitActionsFor(
      input({
        git: git({ files: [file()] }),
        pr: pr(),
        checks: checks({ merge_state: "behind" }),
      }),
    );
    expect(all.map((a) => a.key)).toEqual(["commit-push", "update-branch"]);
    expect(all.filter(isCommitAction).map((a) => a.key)).toEqual(["commit-push"]);
  });
});

describe("checkOutcome", () => {
  it("reads a run the way the desktop's checks section does", () => {
    expect(checkOutcome(run())).toBe("passed");
    expect(checkOutcome(run({ conclusion: "skipped" }))).toBe("passed");
    expect(checkOutcome(run({ conclusion: "failure" }))).toBe("failed");
    expect(checkOutcome(run({ conclusion: "timed_out" }))).toBe("failed");
    expect(checkOutcome(run({ status: "in_progress", conclusion: null }))).toBe("pending");
    expect(checkOutcome(run({ status: "queued", conclusion: null }))).toBe("pending");
  });
});
