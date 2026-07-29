// What autopilot actually did, per checkout — the audit trail for a loop whose
// whole premise is acting while nobody is watching.
//
// The other autopilot maps hold only the present tick: `autopilot` is the live
// state, `autopilotVerdicts` the evidence for the cycle in flight. Both are
// overwritten as the loop moves, so returning to a PR autopilot touched three
// times leaves nothing to read but the agent's chat transcript. A user who can't
// see what was spent on their behalf can't trust the feature, so the driver
// records each decisive effect here as it applies it.
//
// NOT persisted, deliberately — same reasoning as cycles (see the header of
// store/autopilot.ts). A restart drops the cycle machinery and re-derives from
// the live world; a log that outlived it would be the only surviving trace of
// loops the app can no longer reason about, describing agents that may since have
// been archived. What survives a restart is the durable record: the agent's
// transcript and the PR itself.

import type { AutopilotEffect, StuckReason } from "@/autopilot";
import type { DelegationKind } from "@/delegation";
import type { SliceCreator } from "./types";

/** Entries kept per checkout. Sized to hold one complete ladder exhaustion — the
 *  full story behind a single `stuck`: the four rungs' budgets sum to 9 cycles
 *  (`RUNG_BUDGET`), each writing a dispatch plus its outcome, so 18 entries cover
 *  everything autopilot can do between a human's "go" and its handing back, with
 *  slack for the escalation itself. Beyond that the oldest rows are answering a
 *  question nobody is asking, and an unbounded array in a day-long session is
 *  just a leak. */
export const AUTOPILOT_LOG_LIMIT = 20;

/** The effects worth remembering: the ones that spend an agent turn or change
 *  what autopilot will do next. Derived from `AutopilotEffect` so the driver can
 *  pass `effect.do` straight through, and so a new effect can't silently go
 *  unlogged — the `Extract` stops compiling if one of these is renamed.
 *
 *  `wait` is excluded because it is most ticks (every 10s, on every enrolled
 *  checkout): logging it would bury the four events that matter and burn the
 *  bound. `verify` / `await-evidence` are excluded as steps *within* a cycle —
 *  the cycle's own outcome already says how it went. */
export type AutopilotOutcome = Extract<
  AutopilotEffect["do"],
  "dispatch" | "settle" | "retry" | "escalate" | "revive"
>;

/** One thing autopilot did, in the terms a user would ask about it: what
 *  happened, to which rung, on which try, and — when it gave up — why.
 *
 *  Field names and nullability mirror `AutopilotEffect` so the driver's effect
 *  switch can hand its own values over unchanged. */
export interface AutopilotLogEntry {
  /** Epoch ms, PASSED IN by the caller. Nothing in this module reads a clock:
   *  the driver already has the `now` it made the decision with, and an entry
   *  stamped with a second, later clock read would misdate the event it
   *  describes. Same convention as `autopilot.ts` / `readiness.ts`. */
  at: number;
  outcome: AutopilotOutcome;
  /** The rung concerned. Null only for an escalation the ladder raised before
   *  settling on a rung. */
  rung: DelegationKind | null;
  /** 1-based cycle attempt, when the event belongs to a cycle. */
  attempt?: number;
  /** Why autopilot handed the checkout back. Only ever set on an `escalate`. */
  reason?: StuckReason;
}

export interface AutopilotLogSlice {
  /** Per-checkout activity log, keyed by `checkoutKey` (see store/git.ts) —
   *  the same scope as `autopilot` itself, so a multi-repo agent's secondary
   *  checkout keeps its own history rather than sharing the primary's.
   *
   *  Newest first: that is the order the panel reads them in, and it makes
   *  pruning the oldest a single tail slice. */
  autopilotLog: Record<string, AutopilotLogEntry[]>;

  /** Append one event for a checkout, dropping the oldest beyond
   *  `AUTOPILOT_LOG_LIMIT`. Append-only by design — nothing edits or reorders a
   *  recorded entry, so what the user reads is what happened. */
  recordAutopilotEvent: (key: string, entry: AutopilotLogEntry) => void;
}

export const createAutopilotLogSlice: SliceCreator<AutopilotLogSlice> = (set) => ({
  autopilotLog: {},

  recordAutopilotEvent: (key, entry) =>
    set((s) => ({
      autopilotLog: {
        ...s.autopilotLog,
        // Unlike the other autopilot actions, this one does NOT require an
        // existing entry: the log is the record of what happened, so an event
        // must survive the unenroll that raced it.
        [key]: [entry, ...(s.autopilotLog[key] ?? [])].slice(0, AUTOPILOT_LOG_LIMIT),
      },
    })),
});
