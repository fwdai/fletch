// InFlightRail/select.ts — the board's pulse, derived. A pure function that
// joins state the board already holds (its rows, the runs behind them, the
// review poll's answers) into one ordered list of what is moving right now. No
// store, no IPC, no React, and no clock — so the join, the ordering and the
// wording are unit-testable in isolation (select.test.ts) and the rail is a thin
// renderer over this.
//
// Modelled on NeedsYou/select.ts, its sibling strip, and deliberately narrower:
// that one answers "what is stopped and yours to unstick", this one answers "what
// is in motion". Nothing here is a decision, so there is no bucket ordering and
// no action per entry beyond the jump every entry shares.
//
// Every word an entry says is borrowed, never restated: a pause is named by
// `pausedLabel` (the item card's chip and the sidebar badge use the same one) and
// a PR's gate by `mergeGate.ts` through `reviewGate`. A rail that called
// "review required" something else would be the drift those modules exist to
// prevent.

import type { RoadmapItem, RoadmapItemReview, WfRun } from "@/api";
import type { MergeGateTone } from "@/mergeGate";
import { pausedLabel } from "@/workflows/run/status";
import { reviewGate } from "../useItemReviews";

/** Which half of the pipeline an entry is in. `active` is in the build lane (not
 *  necessarily moving — see `building`), `in_review` is built and waiting on its
 *  PR — the two statuses the user did not put the item in and cannot move by
 *  hand. */
export type RailKind = "active" | "in_review";

export interface RailEntry {
  /** The item id: one entry per in-flight row, so this is also the render key. */
  id: string;
  kind: RailKind;
  code: string;
  title: string;
  /** What it is doing, in the vocabulary its own card already uses — a run's
   *  pause, a PR's merge gate, or the plain word when there is nothing more
   *  specific to say yet. */
  state: string;
  /** Severity for the state chip, the shared `MergeGateTone` so an `in_review`
   *  entry and its card's gate chip cannot read differently. */
  tone: MergeGateTone;
  /** Whether tokens are actually being spent on this row right now. Only an
   *  `active` row with a live run is; a hold, a pause or a run that already ended
   *  is not, and the strip's count must not add it to "being built". */
  building: boolean;
  /** The run building an `active` row, when one is recorded — the drainer flips
   *  the status a beat before the run exists, so this can be absent on a row
   *  that is legitimately active. */
  runId?: string;
  /** When that run started (ms epoch). The elapsed span is rendered from a clock
   *  in the component: a selector that read `Date.now()` would be untestable and
   *  would re-derive on every tick. */
  startedAt?: number;
}

export interface RailInput {
  /** The rows the board renders. A shipped item has left the board and is not in
   *  flight; everything before `active` hasn't started. */
  items: readonly RoadmapItem[];
  /** This project's runs by id — `useRoadmap.runsById`, which is already scoped
   *  to the project, so a run id that doesn't resolve here simply yields no run
   *  state rather than another board's. */
  runsById: ReadonlyMap<string, WfRun>;
  /** The review poll's answers by item id (`useRoadmap.reviews`). An item absent
   *  from it has no answer yet, which reads as "in review" — never as a clean
   *  gate. */
  reviews: ReadonlyMap<string, RoadmapItemReview>;
}

/** A run row that has stopped for good. These linger in `runsById` — `wf_list_runs`
 *  filters nothing and the drainer settles the item it belongs to on a 15s tick (or
 *  never, if that write fails) — so the rail meets them routinely and must not read
 *  them as motion. */
function isOver(run: WfRun | undefined): boolean {
  return run?.status === "done" || run?.status === "failed" || run?.status === "canceled";
}

/** What an `active` row is doing.
 *
 *  A hold outranks the run: the row was stopped by hand, and whatever its run says
 *  the item is not going anywhere until it is released (the Needs-you strip above
 *  carries that affordance; this strip only has to stop claiming motion).
 *
 *  Then the run's own status, every branch of it. Collapsing "not paused" to
 *  "running" is how a failed run kept a live clock on this strip for a tick — or
 *  forever. `paused` is unconditional: a pause with no recorded reason is still a
 *  pause, not motion. */
function activeState(
  item: RoadmapItem,
  run: WfRun | undefined,
): { state: string; tone: MergeGateTone; building: boolean } {
  if (item.hold_reason) return { state: "held", tone: "attention", building: false };
  switch (run?.status) {
    case "paused":
      return {
        state: run.paused_reason ? `paused — ${pausedLabel(run.paused_reason)}` : "paused",
        tone: "warn",
        building: false,
      };
    case "failed":
      return { state: "run failed", tone: "attention", building: false };
    case "canceled":
      return { state: "run canceled", tone: "warn", building: false };
    // The run is over and the item hasn't caught up yet: the drainer moves it to
    // review (or done) on its next tick. "complete" is the run's word for this and
    // would read as a shipped item on a board row, so say what the *item* is doing.
    case "done":
      return { state: "finishing", tone: "info", building: false };
    case "pending":
      return { state: "starting", tone: "info", building: true };
    // `running`, and the row whose run isn't in hand yet — the drainer flips the
    // status a beat before the run exists, and a lookup miss says nothing either
    // way. Both are the board's claim that this is being built, unrefuted.
    default:
      return { state: "running", tone: "info", building: true };
  }
}

/** What an `in_review` row is doing: its PR's merge gate, in the gate's own words.
 *
 *  Except when there is nothing to watch — a URL with no number is exactly what
 *  `merge_sweep::pollable` skips, so no gate will ever arrive and a bland "in
 *  review" would sit there forever. The card says the same thing (and offers "Mark
 *  done"); the rail must not look calmer than the card. */
function reviewState(
  item: RoadmapItem,
  review: RoadmapItemReview | undefined,
): { state: string; tone: MergeGateTone } {
  if (item.pr_url && item.pr_number == null) {
    return { state: "can't watch this PR", tone: "warn" };
  }
  const gate = review ? reviewGate(review) : null;
  return { state: gate?.label ?? "in review", tone: gate?.tone ?? "info" };
}

/** Compose the rail from the board's current state. Board order (rank) within
 *  each half, `active` first: what is being built now sits above what is waiting
 *  to ship, which is the direction the pipeline runs. */
export function buildInFlight(input: RailInput): RailEntry[] {
  const entries: { entry: RailEntry; rank: number }[] = [];

  for (const item of input.items) {
    if (item.status === "active") {
      const run = item.run_id ? input.runsById.get(item.run_id) : undefined;
      entries.push({
        rank: item.rank,
        entry: {
          id: item.id,
          kind: "active",
          code: item.code,
          title: item.title,
          ...activeState(item, run),
          runId: run?.id,
          // No clock on a run that has stopped: a span that keeps counting is the
          // strip asserting work is happening, which is the lie this rail exists to
          // avoid. A held or paused row keeps it — that elapsed time is the cost of
          // the decision the user hasn't made.
          startedAt: isOver(run) ? undefined : run?.created_at,
        },
      });
    } else if (item.status === "in_review") {
      entries.push({
        rank: item.rank,
        entry: {
          id: item.id,
          kind: "in_review",
          code: item.code,
          title: item.title,
          building: false,
          ...reviewState(item, input.reviews.get(item.id)),
        },
      });
    }
  }

  entries.sort(
    (a, b) =>
      (a.entry.kind === b.entry.kind ? 0 : a.entry.kind === "active" ? -1 : 1) ||
      a.rank - b.rank ||
      (a.entry.code < b.entry.code ? -1 : a.entry.code > b.entry.code ? 1 : 0),
  );
  return entries.map((e) => e.entry);
}
