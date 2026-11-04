import { describe, expect, it } from "vitest";
import type { BoxStateEvent, BoxVm } from "@/api";
import { foldVmRows, vmRows } from "./vmRows";

function state(id: string, phase: BoxStateEvent["phase"], detail?: string): BoxStateEvent {
  return { agent_id: id, phase, detail };
}

function vm(id: string, created: string, phase: BoxStateEvent["phase"] = "stopped"): BoxVm {
  return { agent_id: id, name: id, project_id: "p", created_at: created, state: state(id, phase) };
}

const ids = (rows: { vm: BoxVm }[]) => rows.map((row) => row.vm.agent_id);

describe("vmRows", () => {
  it("puts live VMs first, then the rest newest first", () => {
    const rows = vmRows(
      [vm("old", "2026-01"), vm("new", "2026-03"), vm("up", "2026-02", "running")],
      {},
    );
    expect(ids(rows)).toEqual(["up", "new", "old"]);
  });

  it("says a stopped VM is not running", () => {
    expect(vmRows([vm("a", "1")], {})[0].status).toBe("Not running");
  });

  it("spells out any other phase", () => {
    const rows = vmRows([vm("a", "1"), vm("b", "2")], {
      a: state("a", "preparing", "downloading image 43%"),
      b: state("b", "failed"),
    });
    expect(rows.map((row) => row.status)).toEqual(["Preparing · downloading image 43%", "Failed"]);
  });

  it("trusts a later box:state over the list's snapshot", () => {
    const rows = vmRows([vm("a", "1", "running")], { a: state("a", "stopped") });
    expect(rows[0].status).toBe("Not running");
  });
});

describe("foldVmRows", () => {
  const stopped = (n: number) => Array.from({ length: n }, (_, i) => vm(`s${i}`, String(i)));

  it("shows a short list whole", () => {
    expect(foldVmRows(vmRows(stopped(5), {})).folded).toEqual([]);
  });

  it("folds what is past the limit", () => {
    const { shown, folded } = foldVmRows(vmRows(stopped(8), {}));
    expect(shown).toHaveLength(5);
    expect(folded).toHaveLength(3);
  });

  it("never folds a VM that is up", () => {
    const up = Array.from({ length: 7 }, (_, i) => vm(`u${i}`, String(i), "running"));
    const { shown, folded } = foldVmRows(vmRows([...up, ...stopped(2)], {}));
    expect(shown).toHaveLength(7);
    expect(ids(folded)).toEqual(["s1", "s0"]);
  });
});
