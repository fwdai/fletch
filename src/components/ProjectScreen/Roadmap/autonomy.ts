/** The autonomy dial: how much of the pipeline runs without a further click.
 *
 *  Two per-project settings and the words the board says because of them. Pure —
 *  no storage, no React — so the board and the Settings section read one
 *  implementation, and the label logic is testable without a DOM.
 *
 *  Every key and every parse rule here mirrors a Rust constant
 *  (`roadmap/drainer.rs`, `roadmap/review.rs`). The spellings must agree: the
 *  frontend writes these rows and the host reads them, and a `"0"` one side calls
 *  off and the other calls unrecognized is a dial that lies. */

/** Accepting a proposed item lands it `queued` instead of `open`. Default off —
 *  `drainer::AUTOQUEUE_KEY`. */
export const AUTOQUEUE_KEY = "roadmap.autoqueue";

/** How many roadmap runs one project may have in flight — `drainer::MAX_CONCURRENT_KEY`. */
export const MAX_CONCURRENT_KEY = "roadmap.max_concurrent";

/** The PM reviews every settled run — `review::SETTLE_REVIEW_KEY`. Default on. */
export const SETTLE_REVIEW_KEY = "roadmap.settle_review";

/** What an unset concurrency dial means — `drainer::MAX_CONCURRENT_ROADMAP_RUNS`. */
export const DEFAULT_MAX_CONCURRENT = 1;

/** The highest the dial goes — `drainer::MAX_CONCURRENT_ROADMAP_CEILING`. The limit
 *  is the repo, not the machine: parallel runs open parallel PRs into one repo, and
 *  past a handful they conflict with each other faster than one person can review
 *  them. */
export const MAX_CONCURRENT_CEILING = 4;

/** Every setting the concurrency select offers, low to high. */
export const CONCURRENCY_CHOICES = Array.from({ length: MAX_CONCURRENT_CEILING }, (_, i) => i + 1);

/** A stored boolean dial, in the spellings the host recognizes
 *  (`drainer::parse_flag`). An absent row — or one nobody can parse — is
 *  `fallback`: a setting that can't be read is not a mandate in either direction. */
export function flagOn(raw: string | undefined, fallback: boolean): boolean {
  switch (raw?.trim().toLowerCase()) {
    case undefined:
    case "":
      return fallback;
    case "1":
    case "true":
    case "on":
    case "yes":
      return true;
    case "0":
    case "false":
    case "off":
    case "no":
      return false;
    default:
      return fallback;
  }
}

/** A stored concurrency cap, parsed and clamped exactly as the host does
 *  (`drainer::parse_cap`) — so the select shows the number that is actually in
 *  force, including for a row somebody hand-edited to `12`. */
export function parseCap(raw: string | undefined): number {
  const text = raw?.trim() ?? "";
  // Plain digits only, because that is what the host's `usize::from_str` accepts:
  // `Number()` would read "1e3" as a thousand where Rust reads it as garbage, and
  // the two sides disagreeing about a hand-edited row is exactly the bug this
  // mirroring exists to prevent.
  if (!/^\+?\d+$/.test(text)) return DEFAULT_MAX_CONCURRENT;
  const n = Number(text);
  if (n < 1) return DEFAULT_MAX_CONCURRENT;
  return Math.min(n, MAX_CONCURRENT_CEILING);
}

/** What the accept actions on a proposal say, and how many there are.
 *
 *  With the dial off there are two: the plain accept, and the one-click
 *  "Accept & queue" (which sends `queue: true` — the same backend decision the
 *  dial takes). With the dial on there is one, and it is *labelled* for what it
 *  does: the primary accept already queues, so a button still saying "Accept"
 *  would understate it. */
export function acceptActions(
  autoqueue: boolean,
  /** The gesture in the caller's words — "Accept" on a card, "Accept all" on the
   *  batch bar. */
  verb: string,
): { primary: string; queue: string | null } {
  const both = `${verb} & queue`;
  return autoqueue ? { primary: both, queue: null } : { primary: verb, queue: both };
}
