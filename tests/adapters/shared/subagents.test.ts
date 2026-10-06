import { describe, expect, it } from "vitest";
import { prOrigins, routeToChild, withSubagentRouting } from "@/adapters/shared/subagents";
import type { ChatItem, RawEvent } from "@/adapters/types";

// A toy top-level reducer: every event appends an agent_message of its text.
const reduceTop = (prev: ChatItem[], ev: RawEvent): ChatItem[] => [
  ...prev,
  { kind: "agent_message", text: String(ev.text) },
];
const call = (id: string, children?: ChatItem[]): ChatItem => ({
  kind: "tool_call",
  id,
  name: "Agent",
  input: {},
  ...(children ? { children } : {}),
});

describe("routeToChild", () => {
  it("reduces a tagged event into the matching tool_call's children", () => {
    const items = [call("t1"), { kind: "agent_message", text: "main" } as ChatItem];
    const next = routeToChild(items, "t1", { text: "hi" }, reduceTop);
    expect(next[0]).toMatchObject({ id: "t1", children: [{ kind: "agent_message", text: "hi" }] });
    expect(next[1]).toBe(items[1]);
  });

  it("finds a tool_call nested inside another's children", () => {
    const items = [call("outer", [call("inner")])];
    const next = routeToChild(items, "inner", { text: "deep" }, reduceTop);
    expect(next[0]).toMatchObject({
      children: [{ id: "inner", children: [{ kind: "agent_message", text: "deep" }] }],
    });
  });

  it("returns the same array when no tool_call matches, so nothing leaks", () => {
    const items = [call("t1")];
    expect(routeToChild(items, "missing", { text: "x" }, reduceTop)).toBe(items);
  });
});

describe("prOrigins", () => {
  const url = (n: number) => `https://github.com/o/r/pull/${n}`;
  const result = (content: unknown, id = "r"): ChatItem => ({
    kind: "tool_result",
    tool_use_id: id,
    content,
  });
  // How `cat` prints the `open_pr` mailbox response.
  const response = (n: number) =>
    `{"id":"x","ok":true,"exit_code":0,"stdout":"${url(n)}","stderr":""}`;

  it("credits the PR in a sub-agent's mailbox response to its launching call", () => {
    const log = [call("a", [result(response(12))]), call("b", [result(response(13))])];
    expect([...prOrigins(log)]).toEqual([
      [12, "a"],
      [13, "b"],
    ]);
  });

  it("credits a nested sub-agent's PR to the top-level launch", () => {
    const log = [call("top", [call("inner", [result(response(7))])])];
    expect(prOrigins(log).get(7)).toBe("top");
  });

  it("ignores the main agent's own results and the sub-agent's final report", () => {
    const log = [call("a", []), result(response(3), "a"), result(response(4))];
    expect(prOrigins(log).size).toBe(0);
  });

  it("reads Claude's text blocks", () => {
    const blocks = [{ type: "text", text: response(21) }];
    expect(prOrigins([call("a", [result(blocks)])]).get(21)).toBe("a");
  });

  it("does not credit a URL merely printed by other output", () => {
    // `gh pr list` / `gh pr view` print a sibling's URL, alone on its line too.
    const listed = `#9\tfix: thing\tOPEN\nsee ${url(9)} for details`;
    const viewed = `${url(10)}\n`;
    const log = [call("a", [result(listed)]), call("b", [result(viewed)])];
    expect(prOrigins(log).size).toBe(0);
  });

  it("keeps the first opener when a later thread repeats the response", () => {
    const log = [call("a", [result(response(5))]), call("b", [result(response(5))])];
    expect(prOrigins(log).get(5)).toBe("a");
  });

  it("reads nothing from content that is neither text nor text blocks", () => {
    const circular: Record<string, unknown> = { output: response(6) };
    circular.self = circular;
    expect(prOrigins([call("a", [result(circular)])]).size).toBe(0);
  });
});

describe("withSubagentRouting", () => {
  const reduce = withSubagentRouting(reduceTop);

  it("routes only events carrying a non-empty parent_tool_use_id", () => {
    let items = reduce([], { text: "top" });
    items = reduce(items, { text: "also top", parent_tool_use_id: "" });
    expect(items.map((i) => i.kind)).toEqual(["agent_message", "agent_message"]);
    items = reduce([call("t1")], { text: "child", parent_tool_use_id: "t1" });
    expect(items).toEqual([call("t1", [{ kind: "agent_message", text: "child" }])]);
  });
});
