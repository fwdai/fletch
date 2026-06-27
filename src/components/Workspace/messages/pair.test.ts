import { describe, expect, it } from "vitest";
import type { ChatItem } from "../../../store";
import { pairToolItems } from "./pair";

const call = (id: string, name = "Bash", input: unknown = ""): ChatItem => ({
  kind: "tool_call",
  id,
  name,
  input,
});

const result = (toolUseId: string, content: unknown = "ok", isError = false): ChatItem => ({
  kind: "tool_result",
  tool_use_id: toolUseId,
  content,
  is_error: isError,
});

const user = (text: string): ChatItem => ({ kind: "user_message", text });
const agent = (text: string): ChatItem => ({ kind: "agent_message", text });

describe("pairToolItems", () => {
  it("returns plain items unchanged", () => {
    const items = [user("hi"), agent("hello")];
    expect(pairToolItems(items)).toEqual(items);
  });

  it("pairs adjacent tool_call + tool_result", () => {
    const c = call("1");
    const r = result("1", "out");
    const out = pairToolItems([c, r]);
    expect(out).toEqual([{ kind: "tool_pair", call: c, result: r }]);
  });

  it("pairs across intervening items", () => {
    const c = call("1");
    const r = result("1", "out");
    const out = pairToolItems([c, agent("thinking..."), r]);
    expect(out).toEqual([{ kind: "tool_pair", call: c, result: r }, agent("thinking...")]);
  });

  it("renders an in-flight tool_call with null result", () => {
    const c = call("1");
    const out = pairToolItems([c]);
    expect(out).toEqual([{ kind: "tool_pair", call: c, result: null }]);
  });

  it("passes through orphan tool_result", () => {
    const r = result("nope", "stale");
    const out = pairToolItems([r]);
    expect(out).toEqual([r]);
  });

  it("matches each call to its own result by id", () => {
    const c1 = call("1", "Bash", "ls");
    const c2 = call("2", "Read", { file_path: "/x" });
    const r2 = result("2", "file body");
    const r1 = result("1", "listing");
    const out = pairToolItems([c1, c2, r2, r1]);
    expect(out).toEqual([
      { kind: "tool_pair", call: c1, result: r1 },
      { kind: "tool_pair", call: c2, result: r2 },
    ]);
  });
});
