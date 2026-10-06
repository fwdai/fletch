// When the focused checkouts' PR detail is worth one read. The webview does not
// poll a PR: the host's watcher emits `pr:state_changed` / `pr:checks_changed` /
// `pr:threads_changed` on change. What it cannot do is tell a window about a PR
// the window has nothing for yet — so `gitSync` reads once, decided here.

import type { GitSlice } from "./git";

/** What the store holds for one focused checkout's PR — just enough to decide
 *  whether it needs a read. `number` is `undefined` while no state is cached,
 *  `null` for "confirmed: no PR". */
export interface FocusedPr {
  key: string;
  number: number | null | undefined;
  open: boolean;
  checks: boolean;
  threads: boolean;
}

/** Which focused checkouts need a one-shot `get_pr_live` and which a
 *  `get_pr_threads`, given what the store holds and the PR number each was last
 *  seen with (`seen`, updated in place).
 *
 *  A read is owed when the cache is cold — no state; no checks for an open PR
 *  (a settled one has none to show); no threads. A change of the PR under a
 *  checkout (a switch, or a PR just opened) owes only its threads: the focus
 *  event already brought the set's checks for it, and a PR the set has no
 *  checks for (one just opened) is an open PR without checks, so it gets its
 *  live read by the first rule. Everything else the watcher's events keep
 *  current. */
export function focusedPrReads(
  prs: FocusedPr[],
  seen: Map<string, number | null>,
): { live: string[]; threads: string[] } {
  const live: string[] = [];
  const threads: string[] = [];
  for (const pr of prs) {
    const before = seen.get(pr.key);
    const changed =
      before !== undefined && pr.number !== undefined && pr.number !== null && pr.number !== before;
    if (pr.number !== undefined) seen.set(pr.key, pr.number);
    if (pr.number === undefined || (pr.open && !pr.checks)) live.push(pr.key);
    if (!pr.threads || changed) threads.push(pr.key);
  }
  return { live, threads };
}

type PrMaps = Pick<GitSlice, "prStates" | "prChecks" | "prComments">;

const FIELD = "\n";

/** One string per checkout carrying everything `focusedPrReads` looks at, so a
 *  shallow-compared selector re-runs its effect only when one of those moves —
 *  not on every PR write in the fleet. Round-trips through
 *  [`parsePrSignature`]. */
export function prSignature(s: PrMaps, key: string): string {
  const state = s.prStates[key];
  const number = state === undefined ? "" : String(state?.number ?? "null");
  const flags = [state?.state === "open", key in s.prChecks, key in s.prComments]
    .map((f) => (f ? "1" : "0"))
    .join("");
  return [key, number, flags].join(FIELD);
}

export function parsePrSignature(sig: string): FocusedPr {
  const [key, number, flags] = sig.split(FIELD);
  return {
    key,
    number: number === "" ? undefined : number === "null" ? null : Number(number),
    open: flags[0] === "1",
    checks: flags[1] === "1",
    threads: flags[2] === "1",
  };
}
