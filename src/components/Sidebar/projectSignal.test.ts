import { describe, expect, it } from "vitest";
import type { AgentRecord, WfRun } from "@/api";
import { projectSignal, strongestSignal } from "./projectSignal";

const agent = (id: string, status: AgentRecord["status"]) => ({ id, status }) as AgentRecord;
const run = (status: WfRun["status"]) => ({ status }) as WfRun;
const NONE = { pending: {}, unseen: {} };

describe("projectSignal", () => {
  it("is nothing for a quiet project", () => {
    expect(projectSignal({ agents: [agent("a", "idle")], runs: [] }, NONE)).toBeNull();
  });

  it("is running while an agent or a workflow run works", () => {
    expect(projectSignal({ agents: [agent("a", "spawning")], runs: [] }, NONE)).toBe("running");
    expect(projectSignal({ agents: [], runs: [run("running")] }, NONE)).toBe("running");
  });

  it("is waiting once a working agent pauses on a question", () => {
    const sources = { pending: { a: { tool: "AskUserQuestion" } }, unseen: {} };
    expect(projectSignal({ agents: [agent("a", "running")], runs: [] }, sources)).toBe("waiting");
  });

  it("is unseen for an idle agent with results nobody opened", () => {
    const sources = { pending: {}, unseen: { a: true } };
    expect(projectSignal({ agents: [agent("a", "idle")], runs: [] }, sources)).toBe("unseen");
  });

  it("lets what needs the user outrank what is busy", () => {
    const sources = { pending: {}, unseen: { a: true } };
    const agents = [agent("b", "running"), agent("a", "idle")];
    expect(projectSignal({ agents, runs: [] }, sources)).toBe("unseen");
  });
});

describe("strongestSignal", () => {
  it("picks waiting over unseen over running", () => {
    expect(strongestSignal(["running", null, "unseen"])).toBe("unseen");
    expect(strongestSignal(["unseen", "waiting"])).toBe("waiting");
    expect(strongestSignal([null, undefined])).toBeNull();
  });
});
