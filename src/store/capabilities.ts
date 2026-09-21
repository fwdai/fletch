// What the UI may offer in the environment the user is driving.
//
// A paired host answers a subset of this app's 212 commands — the rows of
// docs/remote-protocol.md's op table — so a control whose op is not on it has
// to say so rather than fail on click. Gating is by op NAME, never by version
// (docs/multi-host-plan.md §5.1): `hostSupports` takes a host that reported a
// `protocol` at its word, and reads a host that reported none as the v2 default
// set, which is today's phone surface.
//
// The local environment is never gated. That is the whole rule for it: with no
// paired hosts every gate below answers `null` and the app is what it was.

import { hostSupports } from "@/remote/types";
import { useAppStore } from "@/store";
import { activeEnvironment, type EnvironmentEntry, LOCAL_ENVIRONMENT_ID } from "./environments";
import type { AppState } from "./types";

/** One thing the UI offers, and why a remote host may not be able to.
 *
 *  `op` is what the control would call: one op, or every op the flow needs from
 *  end to end — a host that answers some of them would otherwise be offered a
 *  control that fails halfway through. `null` means the blocker is on THIS
 *  side, not the host's (docs/multi-host-plan.md §5.3, item 9), so the gate
 *  closes for every remote environment regardless of its descriptor. */
interface Gate {
  op: string | readonly string[] | null;
  /** The feature itself, in two or three words — what a list of what this host
   *  cannot do reads as. The `reason` is the sentence; this is the name. */
  label: string;
  reason: string;
}

/** Every gate in the app, so the reasons are written once and read the same
 *  way everywhere. Keep the wording short: it is a tooltip, not a dialog. */
export const GATES = {
  /** Pinning a folder that is already on the host. Every remote route into it
   *  browses the disk first — the native picker cannot see it — so the flow is
   *  `list_dir` then `add_workspace_repo`, and a host answering only one of them
   *  would be offered a picker with nowhere to send the folder. Both have been
   *  on the wire since v2, so this closes only against a narrower host. */
  openProject: {
    op: ["list_dir", "add_workspace_repo"],
    label: "Adding projects",
    reason: "This host is too old to add a project — add it on the host.",
  },
  /** Cloning from GitHub: the same browse for the destination, the clone
   *  itself, and the two `gh_*` reads the repo list and the connect prompt are
   *  built on. */
  cloneProject: {
    op: ["list_dir", "clone_repo", "gh_status", "gh_repo_list"],
    label: "Cloning a repository",
    reason: "This host is too old to clone a repository — clone it on the host.",
  },
  /** Creating a fresh repo: browse for the parent, then create it there.
   *  `create_repo` went on the wire with the project-settings family, so this
   *  closes only against a host from before them. */
  createProject: {
    op: ["list_dir", "create_repo"],
    label: "Creating a project",
    reason: "This host can't create a repository from here yet.",
  },
  /** The project settings page's writes, as one gate: the display name, the
   *  repo list (attach, detach, relocate, label, unpin) and deleting the
   *  project. Every op the page can call, so a host that answers only some of
   *  them is not offered a page whose other half fails on click — they went on
   *  the wire together, so in practice a host has all of them or none.
   *  `list_dir` is in the list for attach and relocate, which browse the host's
   *  disk for the folder first. */
  projectAdmin: {
    op: [
      "list_dir",
      "rename_project",
      "delete_project",
      "project_has_running_agents",
      "attach_repo_to_project",
      "detach_repo_from_project",
      "relocate_repo",
      "set_repo_label",
      "remove_workspace_repo",
    ],
    label: "Project settings",
    reason: "This host is too old to change project settings — change them on the host.",
  },
  sideShell: {
    op: "open_agent_shell",
    label: "Terminals",
    reason: "Terminals aren't available on a remote host yet.",
  },
  runScripts: {
    op: "run_start",
    label: "Running the app",
    reason: "Running the app isn't available on a remote host yet.",
  },
  workflows: {
    op: "wf_list_runs",
    label: "Workflows",
    reason: "This host is too old to run workflows.",
  },
  roadmap: {
    op: "roadmap_create_item",
    label: "The roadmap",
    reason: "This host is too old to drive a roadmap board.",
  },
  /** The desktop's own autopilot ladder (`useAutopilotSync`). `null` for the
   *  same reason `addProject` is: the blocker is on THIS side. Its opt-outs are
   *  rows in this Mac's `settings` / `project_settings`, read through the
   *  local-only `db_*` bridge and keyed by project and agent ids that mean
   *  nothing on another machine, and the verify rung calls `run_verification`,
   *  which is off the wire with the rest of the `run_*` family. Left ticking
   *  against a host it would judge a remote project by a local opt-out and
   *  spend the host's agent turns on it. The *host's* own autonomous loop — the
   *  roadmap queue — is unaffected: it runs on the host, and the board above
   *  drives it. */
  autopilot: {
    op: null,
    label: "Autopilot",
    reason: "Autopilot runs on this Mac, for this Mac's projects.",
  },
  nativeView: {
    op: "switch_view",
    label: "The native terminal view",
    reason: "The native terminal view isn't available on a remote host yet.",
  },
  fork: {
    op: "fork_agent",
    label: "Forking",
    reason: "Forking isn't available on a remote host yet.",
  },
  /** Merging a PR is on the wire, so this closes only against a host from
   *  before the op existed — the same shape as any other added op. */
  mergePr: {
    op: "merge_pr",
    label: "Merging a PR",
    reason: "This host is too old to merge a PR — merge it on GitHub.",
  },
  /** Bringing an archived session back. Also only closed by an older host. */
  restore: {
    op: "restore_agent",
    label: "Restoring a session",
    reason: "This host is too old to restore an archived session.",
  },
  /** The Git panel's working-tree actions. All five are on the wire — they act
   *  inside the agent's checkout, the reach `commit_agent` has — so these close
   *  only against a host from before the ops existed. */
  pull: {
    op: "pull_agent",
    label: "Pulling",
    reason: "This host is too old to pull — pull on the host.",
  },
  rebase: {
    op: "rebase_agent",
    label: "Rebasing",
    reason: "This host is too old to rebase — rebase on the host.",
  },
  stash: {
    op: "stash_agent",
    label: "Stashing",
    reason: "This host is too old to stash — stash on the host.",
  },
  discardChanges: {
    op: "discard_agent_changes",
    label: "Discarding changes",
    reason: "This host is too old to discard changes — discard on the host.",
  },
  abortMerge: {
    op: "abort_merge_agent",
    label: "Aborting a merge",
    reason: "This host is too old to abort the merge — abort on the host.",
  },
  /** The one Git-panel action deliberately withheld rather than pending: it
   *  runs `git branch -D` in the user's real clone, outside every checkout
   *  (docs/remote-protocol.md, "Withheld on policy"). No host advertises the
   *  op, so this closes on every remote environment — and opens by itself if a
   *  later release does add the row. */
  deleteBranch: {
    op: "delete_branch_agent",
    label: "Deleting a branch",
    reason: "Delete the branch on the host — it lives in the project repo, not the session.",
  },
} as const satisfies Record<string, Gate>;

export type GateName = keyof typeof GATES;

/** The ops `gate` needs a host to answer — none at all for a gate this side
 *  closes. Written as a bare string for the common one-op case, so this is
 *  where the two shapes become one. */
export function requiredOps(gate: GateName): readonly string[] {
  const { op } = GATES[gate];
  return op === null ? [] : typeof op === "string" ? [op] : op;
}

/** Why `gate` is closed in `env`, or null when it is open. Pure, so the gate
 *  table is testable without a store or a host.
 *
 *  Every op the flow needs, not just its last one: a host that takes the folder
 *  but cannot list a directory would otherwise be offered a picker that opens
 *  on an error. */
export function gateReason(env: EnvironmentEntry, gate: GateName): string | null {
  if (env.kind === "local") return null;
  const ops = requiredOps(gate);
  if (ops.length > 0 && ops.every((op) => hostSupports(env.protocol, op))) return null;
  return GATES[gate].reason;
}

/** A gate that is closed in one environment: the feature's name and the reason
 *  its control gives, carried together so a list of them can be both counted
 *  and read out. */
export interface ClosedGate {
  name: GateName;
  label: string;
  reason: string;
}

/** Every gate closed in `env`, in table order. Empty for the local environment
 *  and for a host that answers everything this app asks of it.
 *
 *  Membership in `protocol.ops`, never a version comparison — `gateReason` is
 *  the only judge here, so the list and the controls themselves can never
 *  disagree (src/remote/types.ts, `hostSupports`). */
export function closedGates(env: EnvironmentEntry): ClosedGate[] {
  return (Object.keys(GATES) as GateName[]).flatMap((name) => {
    const reason = gateReason(env, name);
    return reason ? [{ name, label: GATES[name].label, reason }] : [];
  });
}

/** What a paired host's row adds to its connection state: one line naming what
 *  this host cannot do, and the tooltip spelling it out with both versions.
 *  Null when there is nothing to say. */
export interface HostSkew {
  closed: ClosedGate[];
  /** The compact line — the closed features by name while there are few of
   *  them, a count once a list would be longer than the row. */
  summary: string;
  /** Every closed gate's reason, one per line, then the two versions. The
   *  versions are reported, never compared: they answer "is this host old?"
   *  for a human, while the list above is derived from the op table. */
  tip: string;
}

/** Name them while the list is short: two reads faster than "2 features". */
const NAMED_LIMIT = 2;

export function hostSkew(env: EnvironmentEntry, clientVersion: string): HostSkew | null {
  if (env.kind === "local") return null;
  // Only a host that has answered a handshake has said what it answers. Reading
  // a missing descriptor as the v2 default set is right for a *control* — it
  // fails closed — but here it would accuse a host that has simply not been
  // greeted yet of gaps it may not have.
  if (env.connection !== "connected") return null;
  // The host's own answer only. A gate with no op is closed by *this* side —
  // autopilot judges a project by this Mac's own opt-out rows — and saying it is
  // "unavailable on this host" would blame the wrong machine; the control that
  // is gated says so itself, where the user is trying to use it.
  const closed = closedGates(env).filter((g) => requiredOps(g.name).length > 0);
  if (closed.length === 0) return null;
  const summary =
    closed.length > NAMED_LIMIT
      ? `${closed.length} features unavailable on this host`
      : `${closed.map((g) => g.label).join(" and ")} unavailable on this host`;
  const tip = [
    ...closed.map((g) => `${g.label}: ${g.reason}`),
    `Host ${env.appVersion ?? "version unknown"} · this app ${clientVersion}`,
  ].join("\n");
  return { closed, summary, tip };
}

/** The host's own version, for the rows that identify it. Null for the local
 *  environment and for a host that has not reported one (nothing has been
 *  greeted yet, or the handshake predates the field). */
export function hostVersionLabel(env: EnvironmentEntry): string | null {
  return env.kind === "remote" && env.appVersion ? `v${env.appVersion}` : null;
}

/** How an environment's connection is doing, in words. Here beside the gate
 *  reasons because it is the same kind of thing — the short sentence a surface
 *  shows instead of the control it can't offer — and because two surfaces say
 *  it (the switcher, and the main pane while a host has nothing to show).
 *
 *  `retrying` is what separates a host that will come back on its own from one
 *  only the user can fix: the client schedules no retry behind "this device is
 *  not paired any more" or "remote access is switched off". */
export function connectionLabel(env: EnvironmentEntry): string {
  if (env.kind === "local") return "this machine";
  switch (env.connection) {
    case "connected":
      return "connected";
    case "connecting":
      return "connecting…";
    default:
      return env.retrying ? "reconnecting…" : (env.error ?? "offline");
  }
}

/** The entry the UI is driving, falling back to the local one — the same answer
 *  `activeEnvironment()` gives outside React, as a selector. */
export const activeEntry = (s: AppState): EnvironmentEntry =>
  s.environments[s.activeEnvironmentId] ?? s.environments[LOCAL_ENVIRONMENT_ID];

/** Why `gate` is closed in the active environment, or null when it is open —
 *  the one thing a component needs to decide between rendering a control,
 *  rendering it disabled, or leaving it out. */
export function useGate(gate: GateName): string | null {
  return useAppStore((s) => gateReason(activeEntry(s), gate));
}

/** [`gateReason`] for the environment the user is driving, outside React — a
 *  store action refusing a write the button is already disabled for, so a
 *  keyboard path (Enter in a field, a blur that saves) cannot slip past the
 *  control. The same backstop `useGitActions.runAction` applies. */
export function activeGateReason(gate: GateName): string | null {
  return gateReason(activeEnvironment(), gate);
}

/** [`useGate`] for a gate picked at render time — the Git panel's action bar
 *  chooses one by the selected action's key. `undefined` means the control
 *  calls nothing a host could refuse, so it is never gated. */
export function useGateFor(gate: GateName | undefined): string | null {
  return useAppStore((s) => (gate ? gateReason(activeEntry(s), gate) : null));
}
