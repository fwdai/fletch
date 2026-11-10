// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from "vitest";
import type { AgentRecord, FileStatus, GitState } from "@/api";
import { useAppStore } from "@/store";
import { checkoutKey } from "@/store/git";
import { act, renderHook } from "@/test/render";
import { aggregateGit, type CheckoutGit, useAgentGit } from "./useAgentGit";

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

  it("drops the last good state of a checkout that has since been refused", () => {
    // The store keeps the state it had when the refusal began.
    const stale = state(4, "src/old.ts");
    const multi = aggregateGit([
      checkout("web", { state: state(1, "a.ts") }),
      checkout("api", { state: stale, blocked: ["core.fsmonitor"] }),
    ]);
    expect(multi.files?.map((f) => f.path)).toEqual(["web/a.ts"]);
    expect(multi).toMatchObject({ ahead: 1, blocked: ["api"] });

    const single = aggregateGit([checkout("app", { state: stale, blocked: ["diff.external"] })]);
    expect(single).toMatchObject({ answered: true, files: [], ahead: 0, blocked: ["app"] });
  });
});

describe("useAgentGit", () => {
  beforeEach(() => useAppStore.setState(useAppStore.getInitialState(), true));

  const agentWith = (...subdirs: string[]) =>
    ({ id: "a1", repos: subdirs.map((subdir) => ({ subdir })) }) as AgentRecord;
  const keysOf = (agent: AgentRecord) =>
    agent.repos.map((repo, i) => checkoutKey(agent.id, i === 0 ? undefined : repo.subdir));
  const setGit = (key: string, git: GitState) =>
    act(() => useAppStore.setState((s) => ({ gitStates: { ...s.gitStates, [key]: git } })));

  /** The hook mounted for `agent`, with how many times it has rendered. */
  const mount = (agent: AgentRecord) => {
    const renders = { count: 0 };
    const hook = renderHook(() => {
      renders.count += 1;
      return useAgentGit(agent);
    });
    return { ...hook, renders };
  };

  // A selector that returns a new value for the same state never gets this
  // far: React gives up on the render with "Maximum update depth exceeded".
  it.each([
    ["one checkout", agentWith("app")],
    ["several checkouts", agentWith("web", "api")],
  ])("settles for an agent with %s", (_, agent) => {
    const { result } = mount(agent);
    expect(result.current).toMatchObject({ answered: false, files: null, ahead: 0 });
  });

  it("follows the primary checkout's state in the store", () => {
    const agent = agentWith("app");
    const { result } = mount(agent);
    const git = state(2, "src/a.ts");
    setGit(keysOf(agent)[0], git);
    expect(result.current).toMatchObject({ answered: true, ahead: 2, blocked: [] });
    expect(result.current.files).toBe(git.files);
  });

  it("answers for every checkout once each has a state or a refusal", () => {
    const agent = agentWith("web", "api");
    const [web, api] = keysOf(agent);
    const { result } = mount(agent);
    setGit(web, state(1, "a.ts"));
    expect(result.current).toMatchObject({ answered: false, files: null });
    act(() => useAppStore.setState({ gitBlocked: { [api]: ["core.fsmonitor"] } }));
    expect(result.current).toMatchObject({ answered: true, ahead: 1, blocked: ["api"] });
    expect(result.current.files?.map((f) => f.path)).toEqual(["web/a.ts"]);
  });

  it("keeps its answer, and its render, across a store write about something else", () => {
    const agent = agentWith("web", "api");
    const { result, renders } = mount(agent);
    for (const key of keysOf(agent)) setGit(key, state(0, "a.ts"));
    const before = { answer: result.current, renders: renders.count };
    setGit("another-agent", state(3, "b.ts"));
    act(() => useAppStore.setState((s) => ({ rightCollapsed: !s.rightCollapsed })));
    expect(result.current).toBe(before.answer);
    expect(renders.count).toBe(before.renders);
  });
});
