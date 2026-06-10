import { describe, expect, it } from "vitest";
import { primaryFor, secondaryFor } from "./primaryActions";

const base = { prNumber: 7, base: "main" };

describe("pr-open primary by merge_state (§7)", () => {
  it("clean → green Merge PR", () => {
    const p = primaryFor("pr-open", { ...base, mergeState: "clean" });
    expect(p.key).toBe("merge");
    expect(p.tone).toBe("success");
    expect(p.statusKind).toBe("ready");
    expect(p.statusLabel).toContain("ready to merge");
  });

  it("unstable → Merge PR enabled, warn status", () => {
    const p = primaryFor("pr-open", { ...base, mergeState: "unstable", checksFailed: 1 });
    expect(p.key).toBe("merge");
    expect(p.tone).toBeUndefined();
    expect(p.statusKind).toBe("warn");
  });

  it("blocked with failing checks → Fix with agent", () => {
    const p = primaryFor("pr-open", { ...base, mergeState: "blocked", checksFailed: 2 });
    expect(p.key).toBe("agent-fix");
    expect(p.statusKind).toBe("attention");
    expect(p.statusLabel).toContain("2 checks failing");
  });

  it("blocked with no failing checks → review gate, View on GitHub", () => {
    const p = primaryFor("pr-open", { ...base, mergeState: "blocked", checksFailed: 0 });
    expect(p.key).toBe("view-pr");
    expect(p.statusLabel).toContain("review");
  });

  it("behind / dirty → Update branch with agent", () => {
    for (const ms of ["behind", "dirty"] as const) {
      const p = primaryFor("pr-open", { ...base, mergeState: ms });
      expect(p.key).toBe("agent-update-branch");
      expect(p.statusKind).toBe("attention");
    }
  });

  it("unknown / has_hooks → Merge with checking status", () => {
    for (const ms of ["unknown", "has_hooks"] as const) {
      const p = primaryFor("pr-open", { ...base, mergeState: ms });
      expect(p.key).toBe("merge");
      expect(p.statusLabel).toContain("checking");
    }
  });

  it("no checks data falls back to mergeable-only behavior", () => {
    const ok = primaryFor("pr-open", { ...base, mergeable: true });
    expect(ok.key).toBe("merge");
    expect(ok.statusLabel).toContain("no conflicts");
    const blocked = primaryFor("pr-open", { ...base, mergeable: false });
    expect(blocked.key).toBe("merge");
    expect(blocked.statusKind).toBe("attention");
  });
});

describe("pr-open secondary menu", () => {
  it("offers merge as an alternate and update-branch when behind", () => {
    const keys = secondaryFor("pr-open", { ...base, mergeState: "behind" }).map((s) => s.key);
    expect(keys).toContain("merge");
    expect(keys).toContain("agent-update-branch");
    expect(keys).toContain("view-pr");
  });

  it("offers agent-fix when checks are failing", () => {
    const keys = secondaryFor("pr-open", { ...base, mergeState: "unstable", checksFailed: 1 }).map((s) => s.key);
    expect(keys).toContain("agent-fix");
  });

  it("delegated open-pr is the pushed-state primary", () => {
    expect(primaryFor("pushed", { ahead: 2 }).key).toBe("agent-open-pr");
    const keys = secondaryFor("pushed", {}).map((s) => s.key);
    expect(keys).toContain("open-pr"); // direct auto-fill alternate
  });
});
