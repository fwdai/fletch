import { describe, expect, it } from "vitest";
import type { AgentRecord, BoxStateEvent } from "@/api";
import { foldVmRows, vmRows } from "./vmRows";

function agent(id: string, created: string, over: Partial<AgentRecord> = {}): AgentRecord {
  return { id, created_at: created, sandbox_engine: "box", ...over } as AgentRecord;
}

function state(id: string, phase: BoxStateEvent["phase"], detail?: string): BoxStateEvent {
  return { agent_id: id, phase, detail };
}

const ids = (rows: { agent: AgentRecord }[]) => rows.map((row) => row.agent.id);

describe("vmRows", () => {
  it("lists only live workspaces that run in a VM", () => {
    const rows = vmRows(
      [
        agent("vm", "1"),
        agent("mac", "2", { sandbox_engine: "sandbox-exec" }),
        agent("none", "3", { sandbox_engine: null }),
        agent("gone", "4", { archive: {} as AgentRecord["archive"] }),
      ],
      {},
    );
    expect(ids(rows)).toEqual(["vm"]);
  });

  it("puts live VMs first, then the rest newest first", () => {
    const rows = vmRows(
      [agent("old", "2026-01"), agent("new", "2026-03"), agent("up", "2026-02")],
      { up: state("up", "running") },
    );
    expect(ids(rows)).toEqual(["up", "new", "old"]);
  });

  it("says a VM with no state, or a stopped one, is not running", () => {
    const rows = vmRows([agent("a", "2"), agent("b", "1")], { b: state("b", "stopped") });
    expect(rows.map((row) => row.status)).toEqual(["Not running", "Not running"]);
  });

  it("spells out any other phase", () => {
    const rows = vmRows([agent("a", "1"), agent("b", "2")], {
      a: state("a", "preparing", "downloading image 43%"),
      b: state("b", "failed"),
    });
    expect(rows.map((row) => row.status)).toEqual(["Preparing · downloading image 43%", "Failed"]);
  });
});

describe("foldVmRows", () => {
  const stopped = (n: number) => Array.from({ length: n }, (_, i) => agent(`s${i}`, String(i)));

  it("shows a short list whole", () => {
    expect(foldVmRows(vmRows(stopped(5), {})).folded).toEqual([]);
  });

  it("folds what is past the limit", () => {
    const { shown, folded } = foldVmRows(vmRows(stopped(8), {}));
    expect(shown).toHaveLength(5);
    expect(folded).toHaveLength(3);
  });

  it("never folds a VM that is up", () => {
    const up = Array.from({ length: 7 }, (_, i) => agent(`u${i}`, String(i)));
    const states = Object.fromEntries(up.map((a) => [a.id, state(a.id, "running")]));
    const { shown, folded } = foldVmRows(vmRows([...up, ...stopped(2)], states));
    expect(shown).toHaveLength(7);
    expect(ids(folded)).toEqual(["s1", "s0"]);
  });
});
