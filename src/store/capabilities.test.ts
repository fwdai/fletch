// The gate predicate. Two rules do all the work: the local environment is never
// gated, and a remote one is judged by op name against what the host said it
// answers — or, from a host that said nothing, against the v2 default set.

import { describe, expect, it } from "vitest";
import { V2_DEFAULT_OPS } from "@/remote/types";
import { closedGates, GATES, gateReason, hostSkew, hostVersionLabel } from "./capabilities";
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
    // Added after the descriptor existed, so a host that reports none is too
    // old for them — which is the whole reason they are gates and not calls.
    expect(gateReason(old, "mergePr")).toBe(GATES.mergePr.reason);
    expect(gateReason(old, "restore")).toBe(GATES.restore.reason);
  });

  it("opens the newly exposed ops on a host that advertises them", () => {
    const current = host([...V2_DEFAULT_OPS, "merge_pr", "restore_agent"]);

    expect(gateReason(current, "mergePr")).toBeNull();
    expect(gateReason(current, "restore")).toBeNull();
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

describe("closedGates", () => {
  it("closes nothing on the local environment", () => {
    expect(closedGates(local)).toEqual([]);
  });

  it("lists every closed gate with the reason its control shows", () => {
    const old = host();

    // A host from before the descriptor answers the v2 set, which carries none
    // of the gated ops — so this is the whole table.
    expect(closedGates(old).map((g) => g.name)).toEqual(Object.keys(GATES));
    for (const gate of closedGates(old)) {
      expect(gate.reason).toBe(gateReason(old, gate.name));
      expect(gate.label).toBe(GATES[gate.name].label);
    }
  });

  it("leaves only the local-only gate closed on a host that answers everything", () => {
    const every = host([
      ...V2_DEFAULT_OPS,
      "open_agent_shell",
      "run_start",
      "wf_list_runs",
      "roadmap_list_items",
      "switch_view",
      "fork_agent",
      "merge_pr",
      "restore_agent",
    ]);

    expect(closedGates(every).map((g) => g.name)).toEqual(["addProject"]);
  });
});

describe("hostSkew", () => {
  const CLIENT = "0.7.32";
  const withVersion = (env: EnvironmentEntry, appVersion: string): EnvironmentEntry => ({
    ...env,
    appVersion,
  });

  it("says nothing about the local environment", () => {
    expect(hostSkew(local, CLIENT)).toBeNull();
  });

  it("says nothing about a host that has not answered a handshake yet", () => {
    // No descriptor and not connected: reading the missing one as the v2 set
    // here would accuse a host of gaps it may not have.
    expect(hostSkew({ ...host(), connection: "connecting" }, CLIENT)).toBeNull();
    expect(hostSkew({ ...host(), connection: "error", error: "offline" }, CLIENT)).toBeNull();
  });

  it("says nothing about a connected host that answers every gated op", () => {
    // `addProject` is still closed — the picker is this Mac's — but that is not
    // this host's shortcoming, so the row stays quiet.
    const every = host([
      ...V2_DEFAULT_OPS,
      "open_agent_shell",
      "run_start",
      "wf_list_runs",
      "roadmap_list_items",
      "switch_view",
      "fork_agent",
      "merge_pr",
      "restore_agent",
    ]);

    expect(gateReason(every, "addProject")).not.toBeNull();
    expect(hostSkew(every, CLIENT)).toBeNull();
  });

  it("names the gaps while there are few of them", () => {
    const nearly = host([
      ...V2_DEFAULT_OPS,
      "open_agent_shell",
      "run_start",
      "wf_list_runs",
      "roadmap_list_items",
      "fork_agent",
      "merge_pr",
    ]);

    const skew = hostSkew(withVersion(nearly, "0.7.30"), CLIENT);

    expect(skew?.closed.map((g) => g.name)).toEqual(["nativeView", "restore"]);
    expect(skew?.summary).toBe(
      "The native terminal view and Restoring a session unavailable on this host",
    );
  });

  it("counts them once a list would be too long, and reports both versions", () => {
    const skew = hostSkew(withVersion(host(), "0.7.30"), CLIENT);

    // Eight op-backed gates; `addProject` is this side's and is left out.
    expect(skew?.closed).toHaveLength(8);
    expect(skew?.summary).toBe("8 features unavailable on this host");
    expect(skew?.tip).toContain(`Merging a PR: ${GATES.mergePr.reason}`);
    expect(skew?.tip).not.toContain(GATES.addProject.reason);
    // Reported side by side, never compared: the list above is the op table's
    // answer, not this line's.
    expect(skew?.tip.split("\n").at(-1)).toBe("Host 0.7.30 · this app 0.7.32");
  });

  it("owns up to a host that reported no version", () => {
    expect(hostSkew(host(), CLIENT)?.tip.split("\n").at(-1)).toBe(
      "Host version unknown · this app 0.7.32",
    );
  });
});

describe("hostVersionLabel", () => {
  it("is null for the local environment and for a host that reported none", () => {
    expect(hostVersionLabel({ ...local, appVersion: "0.7.32" })).toBeNull();
    expect(hostVersionLabel(host())).toBeNull();
  });

  it("labels the version the host reported", () => {
    expect(hostVersionLabel({ ...host(), appVersion: "0.7.30" })).toBe("v0.7.30");
  });
});
