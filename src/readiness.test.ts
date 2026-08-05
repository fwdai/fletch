// `readiness.ts` is the single classification of "what's wrong and what fixes
// it". Two surfaces used to answer that independently, which is how they came to
// disagree about what "checks failing" counts. These tests pin the taxonomy, the
// ladder's ordering, and — mechanically — the import rules that keep the module
// portable to Rust.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import type { GitState, PrChecks, PrComment, PrComments, PrState } from "@/api";
import { detectBlockers, type LadderContext, nextRung, type ReadinessInput } from "@/readiness";

function git(over: Partial<GitState> = {}): GitState {
  return {
    branch: "feat",
    parent_branch: "main",
    ahead: 1,
    behind: 0,
    unpushed: 0,
    files: [],
    additions: 0,
    deletions: 0,
    has_origin: true,
    ...over,
  };
}
const file = (kind: GitState["files"][number]["kind"], path = "a.ts") => ({
  path,
  kind,
  staged: false,
  additions: 1,
  deletions: 0,
});
function pr(over: Partial<PrState> = {}): PrState {
  return {
    number: 7,
    url: "https://x",
    state: "open",
    title: "t",
    mergeable: "mergeable",
    ...over,
  };
}
function checks(over: Partial<PrChecks> = {}): PrChecks {
  return {
    merge_state: "clean",
    rollup: "none",
    total: 0,
    passed: 0,
    failed: 0,
    pending: 0,
    required_failing: [],
    runs: [],
    ...over,
  };
}
const comments = (n: number, over: Partial<PrComment> = {}): PrComments => ({
  unresolved: Array.from({ length: n }, (_, i) => ({
    id: `t${i}`,
    author: "bot",
    is_bot: true,
    body: `c${i}`,
    path: null,
    line: null,
    url: "https://x",
    replies: 0,
    we_replied_last: false,
    ...over,
  })),
});

const input = (over: Partial<ReadinessInput> = {}): ReadinessInput => ({
  git: git(),
  pr: null,
  checks: null,
  comments: null,
  ...over,
});
const CTX: LadderContext = { base: "main", commitMode: "commit-pr" };
const kinds = (i: ReadinessInput) => detectBlockers(i).map((b) => b.kind);

describe("detectBlockers", () => {
  it("reports nothing for a clean, pushed, gate-clean proposal", () => {
    expect(kinds(input({ pr: pr(), checks: checks(), comments: comments(0) }))).toEqual([]);
  });

  it("reports unknown state as no blockers, not as 'nothing wrong'", () => {
    // `git: null` means the first read hasn't landed. An empty list here is only
    // safe because the ladder refuses to act on it — see nextRung.
    expect(detectBlockers(input({ git: null }))).toEqual([]);
    expect(nextRung(input({ git: null }), CTX)).toEqual({ do: "wait", why: "unknown-state" });
  });

  it("treats a conflict as its own blocker, not also as uncommitted work", () => {
    // Mid-conflict the files ARE uncommitted, but saying so twice would make the
    // ladder look like it has two problems to fix when it has one. `ahead: 0`
    // keeps `unsubmitted` out of the way so this asserts only the precedence.
    const b = detectBlockers(
      input({ git: git({ ahead: 0, files: [file("conflicted"), file("modified")] }) }),
    );
    expect(b).toEqual([{ kind: "conflicted", paths: ["a.ts"] }]);
  });

  it("counts uncommitted files when nothing is conflicted", () => {
    expect(
      detectBlockers(
        input({ git: git({ ahead: 0, files: [file("modified"), file("added", "b.ts")] }) }),
      ),
    ).toEqual([{ kind: "uncommitted", files: 2 }]);
  });

  it("escalates rather than ignoring a proposal that was closed unlanded", () => {
    expect(kinds(input({ pr: pr({ state: "closed" }) }))).toEqual(["proposal-closed"]);
  });

  it("reports unpushed commits, and unsubmitted only when no proposal exists", () => {
    expect(kinds(input({ git: git({ unpushed: 2 }) }))).toEqual(["unpushed", "unsubmitted"]);
    // A proposal exists → the work is submitted, whatever else is wrong.
    expect(kinds(input({ git: git({ unpushed: 2 }), pr: pr(), checks: checks() }))).toEqual([
      "unpushed",
    ]);
    // A merged proposal covers the work that landed, `ahead` and all — nothing
    // left to propose. A closed one is its own blocker (see below); replacing it
    // is a human call, never a silent re-submit.
    expect(kinds(input({ git: git({ ahead: 3 }), pr: pr({ state: "merged" }) }))).toEqual([]);
    expect(kinds(input({ git: git({ unpushed: 2 }), pr: pr({ state: "closed" }) }))).not.toContain(
      "unsubmitted",
    );
    // But commits made *after* the merge need a follow-up proposal. Only
    // `unpushed` can see them — `ahead` above is stale-base noise.
    expect(kinds(input({ git: git({ unpushed: 2 }), pr: pr({ state: "merged" }) }))).toEqual([
      "unpushed",
      "unsubmitted",
    ]);
  });

  it("maps the merge gate onto diverged / checks / review / draft", () => {
    const gate = (over: Partial<PrChecks>) =>
      kinds(input({ pr: pr(), checks: checks(over), comments: comments(0) }));
    expect(gate({ merge_state: "behind" })).toEqual(["diverged"]);
    expect(gate({ merge_state: "dirty" })).toEqual(["diverged"]);
    expect(gate({ merge_state: "blocked", required_failing: ["test"] })).toEqual([
      "checks-failing",
    ]);
    // Blocked with nothing failing is a pure review gate, not a checks problem.
    expect(gate({ merge_state: "blocked" })).toEqual(["review-required"]);
    expect(gate({ merge_state: "draft" })).toEqual(["draft"]);
    // `unstable` with nothing failing is just "checks still running".
    expect(gate({ merge_state: "unstable", required_failing: [] })).toEqual([]);
    // But a failing check blocks whether or not it shuts the gate. A repo with no
    // required status checks (GitHub's default) reports every failure as
    // `unstable`, and skipping those is how a red PR read as nothing to do.
    expect(gate({ merge_state: "unstable", required_failing: ["rust-test"] })).toEqual([
      "checks-failing",
    ]);
  });

  it("never invents a conflict from a not-yet-computed mergeable verdict", () => {
    // `unknown` is GitHub's "haven't computed it yet" (and every DB snapshot's
    // value). Reading it as a conflict is the overclaim mergeGate.ts warns about.
    expect(kinds(input({ pr: pr({ mergeable: "unknown" }) }))).toEqual([]);
    expect(kinds(input({ pr: pr({ mergeable: "conflicting" }) }))).toEqual(["diverged"]);
  });

  it("carries the failing check names, not a count", () => {
    // The names are what the fix-checks playbook needs; a bare count would make
    // the trigger useless.
    const b = detectBlockers(
      input({
        pr: pr(),
        checks: checks({ merge_state: "blocked", required_failing: ["build", "test (18)"] }),
      }),
    );
    expect(b).toEqual([{ kind: "checks-failing", checks: ["build", "test (18)"] }]);
  });

  it("reads the failing NAMES, never the raw failed count", () => {
    // The drift this module ends: `failed` counts every failing run, so a rerun
    // double-counts, and the fix-checks trigger needs names anyway. A report that
    // claims failures but names none yields no blocker — there is nothing to hand
    // an agent — rather than a phantom one off the count.
    expect(
      kinds(
        input({
          pr: pr(),
          checks: checks({ merge_state: "unstable", failed: 3, required_failing: [] }),
          comments: comments(0),
        }),
      ),
    ).toEqual([]);
  });

  it("reports unresolved review threads only on an open proposal", () => {
    expect(kinds(input({ pr: pr(), checks: checks(), comments: comments(3) }))).toEqual([
      "review-unaddressed",
    ]);
    // Threads on a merged PR are history, not a blocker.
    expect(kinds(input({ pr: pr({ state: "merged" }), comments: comments(3) }))).toEqual([]);
  });
});

describe("nextRung ordering", () => {
  const rung = (i: Partial<ReadinessInput>, ctx: LadderContext = CTX) => nextRung(input(i), ctx);

  it("reconciles the working copy before anything else", () => {
    // Conflicted AND behind AND failing: the local mess comes first, because
    // every later rung would build on a broken tree.
    const r = rung({
      git: git({ files: [file("conflicted")], unpushed: 1 }),
      pr: pr(),
      checks: checks({ merge_state: "dirty", required_failing: ["test"] }),
      comments: comments(2),
    });
    expect(r).toMatchObject({ do: "delegate", kind: "resolve", action: "resolve-conflicts" });
  });

  it("commits in the user's sticky mode, degrading commit-pr to push when a PR is open", () => {
    expect(rung({ git: git({ files: [file("modified")] }) })).toMatchObject({
      kind: "commit-pr",
      params: { base: "main" },
    });
    // A PR already exists — pushing is what updates it, so "open PR" is wrong.
    expect(
      rung({ git: git({ files: [file("modified")] }), pr: pr(), checks: checks() }),
    ).toMatchObject({ kind: "commit-push" });
    // Plain local commit mode carries no base param.
    expect(
      rung({ git: git({ files: [file("modified")] }) }, { base: "main", commitMode: "commit" }),
    ).toMatchObject({ kind: "commit", action: "commit", params: undefined });
  });

  it("proposes the work before asking the forge about it", () => {
    // Unpushed AND unsubmitted: open-pr wins, since it pushes and proposes in one
    // turn — pushing first would spend a whole agent turn for nothing.
    expect(rung({ git: git({ unpushed: 2 }) })).toMatchObject({
      do: "delegate",
      kind: "open-pr",
      params: { base: "main" },
    });
    // Already proposed, just behind on pushes → plain push.
    expect(rung({ git: git({ unpushed: 2 }), pr: pr(), checks: checks() })).toMatchObject({
      kind: "push",
    });
  });

  it("syncs with mainline before trusting a check result", () => {
    const r = rung({
      pr: pr(),
      checks: checks({ merge_state: "behind", required_failing: ["test"] }),
    });
    expect(r).toMatchObject({ kind: "update-branch", params: { base: "main" } });
  });

  it("fixes checks before reading review threads written against them", () => {
    const r = rung({
      pr: pr(),
      checks: checks({ merge_state: "blocked", required_failing: ["build", "test"] }),
      comments: comments(4),
    });
    expect(r).toMatchObject({ kind: "fix-checks", params: { failing: "build, test" } });
  });

  it("escalates what no agent can clear", () => {
    expect(rung({ pr: pr(), checks: checks({ merge_state: "blocked" }) })).toMatchObject({
      do: "escalate",
      blocker: { kind: "review-required" },
    });
    expect(rung({ pr: pr(), checks: checks({ merge_state: "draft" }) })).toMatchObject({
      do: "escalate",
      blocker: { kind: "draft" },
    });
  });

  it("hands unresolved review threads to the agent, with the count", () => {
    expect(
      rung({ pr: pr(), checks: checks({ merge_state: "clean" }), comments: comments(2) }),
    ).toMatchObject({
      do: "delegate",
      kind: "resolve-comments",
      params: { count: "2" },
      blocker: { kind: "review-unaddressed", count: 2 },
    });
  });

  it("escalates a thread it already pushed back on instead of re-arguing it", () => {
    // We had the last word and left it open on purpose. There is no remediation
    // for a disagreement — a person settles it.
    expect(
      rung({
        pr: pr(),
        checks: checks({ merge_state: "clean" }),
        comments: comments(1, { we_replied_last: true }),
      }),
    ).toMatchObject({ do: "escalate", blocker: { kind: "review-disputed", count: 1 } });
  });

  it("works the actionable threads first when some are disputed and some aren't", () => {
    // A mix must not stall on the disagreement: fix what can be fixed, and the
    // disputed one surfaces once nothing else is left.
    const mixed: PrComments = {
      unresolved: [...comments(1, { we_replied_last: true }).unresolved, ...comments(1).unresolved],
    };
    expect(
      rung({ pr: pr(), checks: checks({ merge_state: "clean" }), comments: mixed }),
    ).toMatchObject({ do: "delegate", kind: "resolve-comments", params: { count: "1" } });
  });

  it("merges only on an open gate, and waits while the gate is still computing", () => {
    expect(rung({ pr: pr(), checks: checks({ merge_state: "clean" }) })).toEqual({ do: "merge" });
    // unknown/has_hooks = GitHub still resolving. Never merge off that.
    for (const merge_state of ["unknown", "has_hooks"] as const) {
      expect(rung({ pr: pr(), checks: checks({ merge_state }) })).toEqual({
        do: "wait",
        why: "gate-computing",
      });
    }
    // No checks read at all, and mergeability not computed → also computing.
    expect(rung({ pr: pr({ mergeable: "unknown" }) })).toEqual({
      do: "wait",
      why: "gate-computing",
    });
    // No checks read, but `mergeable` says no conflict: that reports conflict
    // presence only, NOT CI status, so it must NOT auto-merge off zero check
    // knowledge — required checks could be failing or unrun. Nothing blocking,
    // nothing merge-ready either → `ready`, for a human to decide.
    expect(rung({ pr: pr({ mergeable: "mergeable" }) })).toEqual({ do: "ready" });
  });

  it("reports a landed proposal, and a clean local tree with nothing to do", () => {
    expect(rung({ pr: pr({ state: "merged" }) })).toEqual({ do: "landed" });
    // Clean tree, nothing pushed anywhere, no proposal to make.
    expect(rung({ git: git({ ahead: 0 }) })).toEqual({ do: "ready" });
  });

  it("keeps climbing after a merge — landed is not the end of the workspace", () => {
    const merged = pr({ state: "merged" });
    // Uncommitted work → commit it, and (no open proposal now) open a follow-up.
    expect(rung({ git: git({ files: [file("modified")] }), pr: merged })).toMatchObject({
      do: "delegate",
      kind: "commit-pr",
      params: { base: "main" },
    });
    // Already committed → straight to the follow-up proposal.
    expect(rung({ git: git({ unpushed: 1 }), pr: merged })).toMatchObject({
      do: "delegate",
      kind: "open-pr",
    });
  });

  it("is total — every combination yields a rung", () => {
    const states: (PrState["state"] | null)[] = ["open", "merged", "closed", null];
    const gates: PrChecks["merge_state"][] = [
      "clean",
      "blocked",
      "unstable",
      "behind",
      "dirty",
      "draft",
      "has_hooks",
      "unknown",
    ];
    for (const state of states) {
      for (const merge_state of gates) {
        for (const files of [[], [file("modified")], [file("conflicted")]]) {
          for (const unpushed of [0, 1]) {
            const r = nextRung(
              input({
                git: git({ files, unpushed }),
                pr: state ? pr({ state }) : null,
                checks: checks({ merge_state }),
                comments: comments(0),
              }),
              CTX,
            );
            expect(r.do).toBeTruthy();
          }
        }
      }
    }
  });
});

describe("portability to Rust", () => {
  // The loop is meant to move into the supervisor later (frontend polling stops
  // on `document.hidden`, so autopilot would pause exactly when unwatched). That
  // move stays mechanical only while this module imports no runtime, so assert it
  // rather than trusting a comment at the top of the file.
  const source = readFileSync(fileURLToPath(new URL("./readiness.ts", import.meta.url)), "utf8");
  const imports = [...source.matchAll(/^import\s[\s\S]*?from\s+"([^"]+)";$/gm)].map((m) => m[1]);
  // Assert against code, not prose — the header documents these very rules, and
  // naming a banned construct in order to ban it must not trip the guard.
  const code = source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");

  it("imports only from the pure core", () => {
    expect(imports.sort()).toEqual(["@/api", "@/delegation", "@/mergeGate"]);
  });

  it("pulls in no framework, store, or platform runtime", () => {
    for (const forbidden of ["react", "zustand", "@tauri-apps", "@/store", "@/components"]) {
      expect(imports.filter((i) => i.includes(forbidden))).toEqual([]);
    }
  });

  it("reads no clock or randomness, so a pass is reproducible from its inputs", () => {
    // A Rust port must not need a time source; `delegationStep` already takes
    // `now` as a parameter for the same reason.
    expect(code).not.toMatch(/Date\.now|new Date|Math\.random/);
  });
});
