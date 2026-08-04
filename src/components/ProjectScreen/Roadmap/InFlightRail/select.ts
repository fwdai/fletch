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

/** Which half of the pipeline an entry is in. `active` is being built, `in_review`
 *  is built and waiting on its PR — the two statuses the user did not put the item
 *  in and cannot move by hand. */
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

/** What an `active` row's run is doing. Paused outranks everything: the pearl is
 *  still pulsing and nothing is happening, which is the one thing this rail must
 *  not let pass as motion. */
function activeState(run: WfRun | undefined): { state: string; tone: MergeGateTone } {
  if (run?.status === "paused" && run.paused_reason) {
    return { state: `paused — ${pausedLabel(run.paused_reason)}`, tone: "warn" };
  }
  return { state: "running", tone: "info" };
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
          ...activeState(run),
          runId: run?.id,
          startedAt: run?.created_at,
        },
      });
    } else if (item.status === "in_review") {
      const review = input.reviews.get(item.id);
      const gate = review ? reviewGate(review) : null;
      entries.push({
        rank: item.rank,
        entry: {
          id: item.id,
          kind: "in_review",
          code: item.code,
          title: item.title,
          state: gate?.label ?? "in review",
          tone: gate?.tone ?? "info",
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
