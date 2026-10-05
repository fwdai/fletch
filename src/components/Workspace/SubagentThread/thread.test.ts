import { describe, expect, it } from "vitest";
import type { BackgroundTask, ChatItem } from "@/store";
import {
  isSubagentCall,
  resolveThread,
  threadLabel,
  threadResult,
  threadState,
  threadSteps,
} from "./thread";

type ToolCall = Extract<ChatItem, { kind: "tool_call" }>;

function call(id: string, name: string, children?: ChatItem[], input: unknown = {}): ToolCall {
  return { kind: "tool_call", id, name, input, children };
}
function result(toolUseId: string, isError = false): ChatItem {
  return { kind: "tool_result", tool_use_id: toolUseId, content: "ok", is_error: isError };
}
const msg = (text: string): ChatItem => ({ kind: "agent_message", text });

const inner = call("t_inner", "Agent", [msg("deep")], { description: "inner" });
const a = call("t_a", "Agent", [msg("hi"), inner, result("t_inner")], { description: "A" });
const b = call("t_b", "Task", [msg("yo")], { subagent_type: "Explore" });
const log: ChatItem[] = [
  { kind: "user_message", text: "go" },
  call("t_bash", "Bash", undefined, { command: "ls" }),
  result("t_bash"),
  a,
  result("t_a"),
  b,
  result("t_b", true),
];

describe("isSubagentCall", () => {
  it("recognizes Agent/Task by name and anything with children", () => {
    expect(isSubagentCall(a)).toBe(true);
    expect(isSubagentCall(b)).toBe(true);
    expect(isSubagentCall(call("x", "collab.spawn", [msg("child")]))).toBe(true);
    expect(isSubagentCall(call("t_bash", "Bash"))).toBe(false);
    expect(isSubagentCall(msg("not a call"))).toBe(false);
  });
});

describe("resolveThread", () => {
  it("resolves a top-level thread with its siblings in launch order", () => {
    const t = resolveThread(log, ["t_b"]);
    expect(t?.call).toBe(b);
    expect(t?.items).toEqual([msg("yo")]);
    expect(t?.trail).toEqual([b]);
    expect(t?.siblings).toEqual([a, b]);
    expect(t?.index).toBe(1);
  });

  it("resolves a nested thread, with siblings from its own parent log", () => {
    const t = resolveThread(log, ["t_a", "t_inner"]);
    expect(t?.call).toBe(inner);
    expect(t?.items).toEqual([msg("deep")]);
    expect(t?.trail).toEqual([a, inner]);
    expect(t?.siblings).toEqual([inner]);
    expect(t?.index).toBe(0);
  });

  it("is null for an empty path, a missing id, a missing log, or a non-call id", () => {
    expect(resolveThread(log, [])).toBeNull();
    expect(resolveThread(log, ["nope"])).toBeNull();
    expect(resolveThread(undefined, ["t_a"])).toBeNull();
    expect(resolveThread(log, ["t_a", "t_bash"])).toBeNull();
  });
});

describe("threadLabel / threadResult / threadSteps", () => {
  it("prefers the description, then the type, then a generic label", () => {
    expect(threadLabel(a)).toBe("A");
    expect(threadLabel(b)).toBe("Explore");
    expect(threadLabel(call("x", "Agent"))).toBe("sub-agent");
  });

  it("finds the launching call's result in the parent log", () => {
    expect(threadResult(log, b)?.is_error).toBe(true);
    expect(threadResult(log, a)?.is_error).toBe(false);
    expect(threadResult(log, call("none", "Agent"))).toBeNull();
  });

  it("counts prose and tool calls as steps", () => {
    expect(threadSteps([msg("a"), call("c", "Read"), result("c"), msg("b")])).toBe(3);
  });
});

describe("threadState", () => {
  const task = (status: BackgroundTask["status"]): BackgroundTask => ({
    taskId: "k",
    toolUseId: "t",
    description: "",
    taskType: "local_agent",
    backgrounded: true,
    ownedBySubagent: false,
    status,
    startedAt: 0,
    lastActivityAt: 0,
    toolUses: 0,
    totalTokens: 0,
    durationMs: 0,
  });
  const ok = result("t") as Extract<ChatItem, { kind: "tool_result" }>;
  const bad = result("t", true) as Extract<ChatItem, { kind: "tool_result" }>;

  it("lets a background task outrank the launch result", () => {
    expect(threadState(ok, task("running"), false)).toBe("running");
    expect(threadState(ok, task("failed"), false)).toBe("failed");
    expect(threadState(ok, task("completed"), false)).toBe("done");
  });

  it("reads a foreground sub-agent from its result and the parent's liveness", () => {
    expect(threadState(null, undefined, true)).toBe("running");
    expect(threadState(null, undefined, false)).toBe("done");
    expect(threadState(bad, undefined, true)).toBe("failed");
    expect(threadState(ok, undefined, true)).toBe("done");
  });
});
