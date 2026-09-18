// The gate predicate. Two rules do all the work: the local environment is never
// gated, and a remote one is judged by op name against what the host said it
// answers — or, from a host that said nothing, against the v2 default set.

import { describe, expect, it } from "vitest";
import { V2_DEFAULT_OPS } from "@/remote/types";
import { GATES, gateReason } from "./capabilities";
import type { EnvironmentEntry } from "./environments";

const local: EnvironmentEntry = {
  id: "local",
  name: "This Mac",
  kind: "local",
  connection: "connected",
};

const host = (ops?: string[]): EnvironmentEntry => ({
  id: "host-key-1",
  name: "Cloud box",
  kind: "remote",
  connection: "connected",
  ...(ops ? { protocol: { version: 2, ops, events: [], features: [] } } : {}),
});

describe("gateReason", () => {
  it("never gates the local environment", () => {
    for (const gate of Object.keys(GATES) as (keyof typeof GATES)[]) {
      expect(gateReason(local, gate)).toBeNull();
    }
  });

  it("gates the local-only affordances on any remote host, however capable", () => {
    // `add_workspace_repo` IS on the wire; the native folder picker that feeds
    // it is not, so this gate ignores the descriptor entirely.
    const generous = host([...V2_DEFAULT_OPS, "open_agent_shell", "run_start"]);

    expect(GATES.addProject.op).toBeNull();
    expect(gateReason(generous, "addProject")).toBe(GATES.addProject.reason);
  });

  it("reads a host that reported no descriptor as the v2 default set", () => {
    const old = host();

    // Every op-backed gate names something the v2 set does not carry, which is
    // exactly why each one is a gate.
    expect(gateReason(old, "sideShell")).toBe(GATES.sideShell.reason);
    expect(gateReason(old, "runScripts")).toBe(GATES.runScripts.reason);
    expect(gateReason(old, "workflows")).toBe(GATES.workflows.reason);
    expect(gateReason(old, "roadmap")).toBe(GATES.roadmap.reason);
    expect(gateReason(old, "nativeView")).toBe(GATES.nativeView.reason);
    expect(gateReason(old, "fork")).toBe(GATES.fork.reason);
  });

  it("opens a gate the host says it answers", () => {
    const withShell = host([...V2_DEFAULT_OPS, "open_agent_shell"]);

    expect(gateReason(withShell, "sideShell")).toBeNull();
    // …and only that one.
    expect(gateReason(withShell, "runScripts")).toBe(GATES.runScripts.reason);
  });

  it("takes a host at its word even when it drops an op the default set had", () => {
    const narrow = host(["get_workspace"]);

    expect(gateReason(narrow, "sideShell")).toBe(GATES.sideShell.reason);
  });
});
