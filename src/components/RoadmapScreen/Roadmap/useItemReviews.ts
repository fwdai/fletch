// The board's review poll: CI, conflicts and unresolved threads for every
// `in_review` item, keyed by item id.
//
// Why this isn't in `gitSync`. That module is the app's single owner of git/GitHub
// polling, and everything in it is keyed by *checkout* —
// `checkoutKey(agentId, subdir)` — because a checkout is what the Git panel
// renders. A roadmap item has no checkout: the run that built it worked in a
// disposable clone that may already be gone, and what survives is the project's
// primary repo plus the item's `pr_number`. Routing this through the
// checkout-keyed machinery would mean inventing a fake agent id for every card,
// so the read is addressed the backend's way (`roadmap_item_review`, which
// resolves the repo exactly as the merge sweep does) and the answers are keyed
// by item id.
//
// Why it's a poll at all, when the host already watches these PRs. The merge
// sweep asks one question — did it merge — and it asks it forever, with the
// window shut, because the queue must keep draining. The review questions only
// matter while someone is looking at the board, so they live here: mounted with
// the board, gone when it unmounts, and (via `usePoll`) paused while the app is
// hidden. Nothing accumulates.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, type RoadmapItem, type RoadmapItemReview } from "@/api";
import { describeMergeGate, type MergeGate, mergeGateLabel } from "@/mergeGate";
import { usePoll } from "@/util/hooks";

/** How often a mounted board re-reads its in-review PRs.
 *
 *  A minute. Slower than the Git panel's 5s tick because nobody is watching a
 *  roadmap card the way they watch a diff, and because each pass spends a
 *  GraphQL point per item for the threads read (thread resolution has no REST
 *  equivalent). Faster than the host sweep's two minutes because this one *is*
 *  being looked at. `usePoll` fires once immediately, so mounting the board — and
 *  the deps change below when an item enters review — is its own refresh. */
const REVIEW_POLL_MS = 60_000;

/** The items a board polls review state for: `in_review` with a PR number.
 *
 *  Mirrors the backend's `pr_review::watchable` (and the merge sweep's
 *  `pollable`) rather than restating a looser rule: an item with a URL and no
 *  number is unpollable — there is no number to ask GitHub with, and one guessed
 *  out of the URL would answer about someone else's PR.
 *
 *  Returns ids, sorted, so the poll's dependency array is stable across the row
 *  churn a board sees for unrelated reasons (a retitle, a rank drag). */
export function reviewTargets(items: readonly RoadmapItem[]): string[] {
  return items
    .filter((i) => i.status === "in_review" && i.pr_number != null)
    .map((i) => i.id)
    .sort();
}

/** A card's whole reading of one review answer: the shared merge-gate verdict,
 *  its terse phrasing, and the two counts worth stating. */
export interface ReviewGate extends MergeGate {
  /** The gate's own words for `situation`, naming the PR's base branch when the
   *  read told us one. */
  label: string;
  /** Failing *required* checks — the same number that split the gate. */
  failing: number;
  /** Unresolved review threads, whoever is waiting on whom. */
  threads: number;
}

/** Derive the card's gate from a review answer.
 *
 *  Classification and phrasing both come from `mergeGate.ts` — the module that
 *  exists so no surface classifies `MergeState` for itself — and `checksFailed`
 *  is `required_failing.length`, not `checks.failed`, because that is what the
 *  gate's split between "agent-fixable checks" and "a pure review gate" means.
 *  Disagreeing about that is the exact drift readiness.ts calls out. */
export function reviewGate(review: RoadmapItemReview): ReviewGate {
  const failing = review.checks?.required_failing.length ?? 0;
  const gate = describeMergeGate(review.checks?.merge_state ?? null, {
    checksFailed: failing,
    // No `PrState` on this surface (the sweep owns "did it merge"), and the
    // fallback path needs one: `"unknown"` is the honest input — it renders as
    // still-computing rather than as a false conflict or a false all-clear.
    mergeable: "unknown",
  });
  return {
    ...gate,
    label: mergeGateLabel(gate.situation, review.base_ref ?? undefined),
    failing,
    threads: review.comments?.unresolved.length ?? 0,
  };
}

/** Live review state for the board's in-review items, by item id.
 *
 *  An item that has no answer yet (or whose read degraded to `null`) is simply
 *  absent from the map, and the card renders its in-review surface without a
 *  gate — never a false "no checks". Answers are dropped as soon as their item
 *  leaves review, so the map stays board-sized. */
export function useItemReviews(items: readonly RoadmapItem[]) {
  const [reviews, setReviews] = useState<ReadonlyMap<string, RoadmapItemReview>>(() => new Map());
  const targetIds = reviewTargets(items);
  /** The poll's scope as one stable string, so a board re-render that changed
   *  nothing about *which* PRs are in review doesn't re-arm the interval (which
   *  would fire an immediate tick every time). */
  const targetKey = targetIds.join(" ");
  const targets = useMemo(() => (targetKey ? targetKey.split(" ") : []), [targetKey]);

  // Unmount (and a project switch) must not write state from a tick already in
  // flight — the answers would land on the next board's cards.
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const tick = useCallback(async () => {
    if (targets.length === 0) {
      // Nothing under review: clear rather than keep the last board's answers,
      // and make no requests at all.
      setReviews((prev) => (prev.size === 0 ? prev : new Map()));
      return;
    }
    const answers = await Promise.all(
      targets.map(async (id) => {
        try {
          return [id, await api.roadmapItemReview(id)] as const;
        } catch {
          // A background read on a view: a transport failure is not the board's
          // error bar. The card keeps what it last knew.
          return [id, null] as const;
        }
      }),
    );
    if (!alive.current) return;
    setReviews((prev) => {
      const next = new Map<string, RoadmapItemReview>();
      for (const [id, answer] of answers) {
        // A degraded read keeps the previous answer — that is the whole point of
        // the backend's `null`: "nothing to say this round", not "no checks".
        const value = answer ?? prev.get(id);
        if (value) next.set(id, value);
      }
      return next;
    });
  }, [targets]);

  usePoll(tick, REVIEW_POLL_MS, [tick]);

  /** Re-read one item now — after a merge, so the card stops offering to merge
   *  an already-merged PR while it waits for the sweep to ship the row. */
  const refreshReview = useCallback(async (itemId: string) => {
    try {
      const answer = await api.roadmapItemReview(itemId);
      if (!alive.current || !answer) return;
      setReviews((prev) => new Map(prev).set(itemId, answer));
    } catch {
      // Same policy as the tick: a failed refresh leaves the last answer up.
    }
  }, []);

  return { reviews, refreshReview };
}
