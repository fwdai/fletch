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
import { type EnvironmentEntry, LOCAL_ENVIRONMENT_ID } from "./environments";
import type { AppState } from "./types";

/** One thing the UI offers, and why a remote host may not be able to.
 *
 *  `op` is the op the control would call. `null` means the blocker is on THIS
 *  side, not the host's — a native folder picker cannot browse a cloud box's
 *  disk however willing the host is (docs/multi-host-plan.md §5.3, item 9) — so
 *  the gate closes for every remote environment regardless of its descriptor. */
interface Gate {
  op: string | null;
  reason: string;
}

/** Every gate in the app, so the reasons are written once and read the same
 *  way everywhere. Keep the wording short: it is a tooltip, not a dialog. */
export const GATES = {
  /** New Project, "Add project", attach/relocate a repo — all of them open a
   *  native picker on this Mac. `add_workspace_repo` itself is on the wire; the
   *  path to hand it is not. */
  addProject: {
    op: null,
    reason: "Add projects on the host or from your phone.",
  },
  sideShell: {
    op: "open_agent_shell",
    reason: "Terminals aren't available on a remote host yet.",
  },
  runScripts: {
    op: "run_start",
    reason: "Running the app isn't available on a remote host yet.",
  },
  workflows: {
    op: "wf_list_runs",
    reason: "This host is too old to run workflows.",
  },
  roadmap: {
    op: "roadmap_create_item",
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
    reason: "Autopilot runs on this Mac, for this Mac's projects.",
  },
  nativeView: {
    op: "switch_view",
    reason: "The native terminal view isn't available on a remote host yet.",
  },
  fork: {
    op: "fork_agent",
    reason: "Forking isn't available on a remote host yet.",
  },
  /** Merging a PR is on the wire, so this closes only against a host from
   *  before the op existed — the same shape as any other added op. */
  mergePr: {
    op: "merge_pr",
    reason: "This host is too old to merge a PR — merge it on GitHub.",
  },
  /** Bringing an archived session back. Also only closed by an older host. */
  restore: {
    op: "restore_agent",
    reason: "This host is too old to restore an archived session.",
  },
} as const satisfies Record<string, Gate>;

export type GateName = keyof typeof GATES;

/** Why `gate` is closed in `env`, or null when it is open. Pure, so the gate
 *  table is testable without a store or a host. */
export function gateReason(env: EnvironmentEntry, gate: GateName): string | null {
  if (env.kind === "local") return null;
  const { op, reason } = GATES[gate];
  if (op !== null && hostSupports(env.protocol, op)) return null;
  return reason;
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
