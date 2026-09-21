// The gate predicate. Two rules do all the work: the local environment is never
// gated, and a remote one is judged by op name against what the host said it
// answers — or, from a host that said nothing, against the v2 default set.

import { describe, expect, it } from "vitest";
import { V2_DEFAULT_OPS } from "@/remote/types";
import {
  closedGates,
  GATES,
  type GateName,
  gateReason,
  hostSkew,
  hostVersionLabel,
} from "./capabilities";
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

/** Every op a gate names: what a host has to answer for the app to offer it
 *  everything that can be offered remotely. Derived from the table, so a gate
 *  added later cannot quietly fall out of the fixtures below. */
const GATED_OPS: string[] = Object.values(GATES)
  .map((g): string | null => g.op)
  .filter((op): op is string => op !== null);

/** How many gates a host from before the descriptor closes by itself: the
 *  op-backed ones whose op is outside the v2 set it answers. */
const OLD_HOST_CLOSED = GATED_OPS.filter((op) => !V2_DEFAULT_OPS.includes(op)).length;

describe("gateReason", () => {
  it("never gates the local environment", () => {
    for (const gate of Object.keys(GATES) as (keyof typeof GATES)[]) {
      expect(gateReason(local, gate)).toBeNull();
    }
  });

  it("gates the local-only affordances on any remote host, however capable", () => {
    // Autopilot's opt-outs are this Mac's rows and its verify rung is a local
    // script, so no descriptor opens it.
    const generous = host([
      ...V2_DEFAULT_OPS,
      "open_agent_shell",
      "run_start",
      "run_verification",
      "fork_agent",
    ]);

    expect(GATES.autopilot.op).toBeNull();
    expect(gateReason(generous, "autopilot")).toBe(GATES.autopilot.reason);
  });

  it("opens adding a project on any host that takes one", () => {
    // Both halves — browsing the disk (`list_dir`) and pinning the folder
    // (`add_workspace_repo`) — have been on the wire since v2, so even a host
    // that reported no descriptor takes a project.
    expect(gateReason(host(), "addProject")).toBeNull();
    expect(gateReason(host([...V2_DEFAULT_OPS]), "addProject")).toBeNull();
    // A host that says it does not answer the op is taken at its word.
    expect(gateReason(host(["get_workspace"]), "addProject")).toBe(GATES.addProject.reason);
  });

  it("keeps creating a project closed until a host offers `create_repo`", () => {
    // Outside the v2 set, so a host from before the op is closed — and it opens
    // by itself the moment one advertises it.
    expect(gateReason(host(), "createProject")).toBe(GATES.createProject.reason);
    expect(gateReason(host([...V2_DEFAULT_OPS]), "createProject")).toBe(GATES.createProject.reason);
    expect(gateReason(host([...V2_DEFAULT_OPS, "create_repo"]), "createProject")).toBeNull();
  });

  it("gates the whole project settings page on one op", () => {
    // The page's writes went on the wire together, so `rename_project` stands
    // for all of them: a host that answers it answers the repo list, the label
    // and the delete too.
    expect(gateReason(host(), "projectAdmin")).toBe(GATES.projectAdmin.reason);
    expect(gateReason(host([...V2_DEFAULT_OPS]), "projectAdmin")).toBe(GATES.projectAdmin.reason);
    expect(gateReason(host([...V2_DEFAULT_OPS, "rename_project"]), "projectAdmin")).toBeNull();
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
    expect(gateReason(old, "pull")).toBe(GATES.pull.reason);
    expect(gateReason(old, "rebase")).toBe(GATES.rebase.reason);
    expect(gateReason(old, "stash")).toBe(GATES.stash.reason);
    expect(gateReason(old, "discardChanges")).toBe(GATES.discardChanges.reason);
    expect(gateReason(old, "abortMerge")).toBe(GATES.abortMerge.reason);
  });

  it("opens the newly exposed ops on a host that advertises them", () => {
    const current = host([
      ...V2_DEFAULT_OPS,
      "merge_pr",
      "restore_agent",
      "pull_agent",
      "rebase_agent",
      "stash_agent",
      "discard_agent_changes",
      "abort_merge_agent",
    ]);

    expect(gateReason(current, "mergePr")).toBeNull();
    expect(gateReason(current, "restore")).toBeNull();
    expect(gateReason(current, "pull")).toBeNull();
    expect(gateReason(current, "rebase")).toBeNull();
    expect(gateReason(current, "stash")).toBeNull();
    expect(gateReason(current, "discardChanges")).toBeNull();
    expect(gateReason(current, "abortMerge")).toBeNull();
  });

  it("keeps the withheld branch delete closed on a host that answers everything else", () => {
    // `delete_branch_agent` is off the wire on purpose — it writes in the
    // user's real clone, not the agent's checkout — so no host advertises it
    // and this gate never opens remotely, however new the host is.
    const current = host([...V2_DEFAULT_OPS, "pull_agent", "abort_merge_agent"]);

    expect(gateReason(current, "deleteBranch")).toBe(GATES.deleteBranch.reason);
    expect(gateReason(host(), "deleteBranch")).toBe(GATES.deleteBranch.reason);
    // …and is still open on this Mac, where the command is a desktop command.
    expect(gateReason(local, "deleteBranch")).toBeNull();
  });

  it("opens workflows and the roadmap on a host that answers their ops", () => {
    // The whole point of exposing the two families: no client-side rule to
    // change, just the names arriving in the descriptor.
    const current = host([...V2_DEFAULT_OPS, "wf_list_runs", "roadmap_create_item"]);

    expect(gateReason(current, "workflows")).toBeNull();
    expect(gateReason(current, "roadmap")).toBeNull();
  });

  it("keeps the roadmap closed on a host that only has the planning-chat reads", () => {
    // `roadmap_list_items` has been on the wire since the phone's planning
    // chat, so gating the board on it would have opened a tab whose every
    // write failed as `unknown op`. The gate names a board *write* instead.
    const planningOnly = host([
      ...V2_DEFAULT_OPS,
      "roadmap_list_items",
      "roadmap_update_item",
      "roadmap_discard_proposal",
    ]);

    expect(gateReason(planningOnly, "roadmap")).toBe(GATES.roadmap.reason);
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

    // A host from before the descriptor answers the v2 set: everything this
    // side gates, plus every gate whose op landed after that set.
    const expected = (Object.keys(GATES) as GateName[]).filter((name) => {
      const op = GATES[name].op;
      return op === null || !V2_DEFAULT_OPS.includes(op);
    });
    expect(expected.length).toBeGreaterThan(0);
    expect(closedGates(old).map((g) => g.name)).toEqual(expected);
    for (const gate of closedGates(old)) {
      expect(gate.reason).toBe(gateReason(old, gate.name));
      expect(gate.label).toBe(GATES[gate.name].label);
    }
  });

  it("leaves only the local-only gates closed on a host that answers everything", () => {
    const every = host([...V2_DEFAULT_OPS, ...GATED_OPS]);

    // The gates with no op — the blocker is on this side, so no host can open
    // them — derived from the table so adding one does not silently break this.
    const localOnly = (Object.keys(GATES) as GateName[]).filter((g) => GATES[g].op === null);
    expect(localOnly.length).toBeGreaterThan(0);
    expect(closedGates(every).map((g) => g.name)).toEqual(localOnly);
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
    // `autopilot` is still closed — it runs on this Mac — but that is not this
    // host's shortcoming, so the row stays quiet.
    const every = host([...V2_DEFAULT_OPS, ...GATED_OPS]);

    expect(gateReason(every, "autopilot")).not.toBeNull();
    expect(hostSkew(every, CLIENT)).toBeNull();
  });

  it("names the gaps while there are few of them", () => {
    // Everything but the native view and restoring a session.
    const missing = ["switch_view", "restore_agent"];
    const nearly = host([...V2_DEFAULT_OPS, ...GATED_OPS.filter((op) => !missing.includes(op))]);

    const skew = hostSkew(withVersion(nearly, "0.7.30"), CLIENT);

    expect(skew?.closed.map((g) => g.name)).toEqual(["nativeView", "restore"]);
    expect(skew?.summary).toBe(
      "The native terminal view and Restoring a session unavailable on this host",
    );
  });

  it("counts them once a list would be too long, and reports both versions", () => {
    const skew = hostSkew(withVersion(host(), "0.7.30"), CLIENT);

    // Every op-backed gate whose op landed after the v2 set this host answers.
    // `autopilot` is this side's and is left out.
    expect(skew?.closed).toHaveLength(OLD_HOST_CLOSED);
    expect(skew?.summary).toBe(`${OLD_HOST_CLOSED} features unavailable on this host`);
    expect(skew?.tip).toContain(`Merging a PR: ${GATES.mergePr.reason}`);
    expect(skew?.tip).not.toContain(GATES.autopilot.reason);
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
