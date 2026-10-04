// ── Autopilot: move an open PR to the finish line, unattended ────────────────
//
// The loop runs on the host (docs/remote-protocol.md, "Autopilot"): it decides
// when to dispatch a rung, judges each cycle and gives up when a rung stops
// converging. This client only renders what the host reports — the store's
// `autopilot` / `autopilotLog` mirrors — so all that is left here are the
// shapes and numbers that rendering needs.

import type { DelegationKind } from "@/delegation";

/** Cycles one rung gets on one situation before the host gives up on it — the
 *  host's budget, repeated here only so a give-up row can say how many tries
 *  were spent ("Gave up on the failing checks after 3 tries"). */
export const RUNG_BUDGET: Partial<Record<DelegationKind, number>> = {
  "fix-checks": 3,
  resolve: 2,
  "update-branch": 2,
  "resolve-comments": 2,
};

/** `working` spans the agent's turn; `awaiting-evidence` is the gap between the
 *  turn ending and the world having something to say about it. */
export type CyclePhase = "working" | "awaiting-evidence";

/** Why autopilot gave up on a rung. Recorded in the checkout's history and
 *  nowhere else: it is a fact about what autopilot did, not a state the user is
 *  in. Once given up, autopilot waits for the world to change. */
export type GiveUpReason =
  /** The rung's cycle budget for this situation is spent. */
  | "budget-spent"
  /** A cycle ended on a world that had already produced nothing. */
  | "no-progress"
  /** No CI verdict arrived within the host's evidence timeout. */
  | "no-evidence";
