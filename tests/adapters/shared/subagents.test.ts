import { describe, expect, it } from "vitest";
import { routeToChild, withSubagentRouting } from "@/adapters/shared/subagents";
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
