// The pre-flight sequence. The checks themselves run in parallel the moment
// onboarding mounts (`useOnboardingSetup`), usually while the welcome screen
// is still up; this hook only decides how they are *revealed*: one row at a
// time, at a pace a person can follow — each row held as "checking" long
// enough for the panel beside it to show the work (the agents panel scans
// its tiles one by one), a green row seen before the next one starts — and
// pausing on a row that needs a person: no agent installed, git that could
// not be set up, or the GitHub decision. Only when every row has an answer
// does the way into the app appear, so there is never more than one thing
// to do.

import { useCallback, useEffect, useState } from "react";
import type { OnboardingSetup } from "../useSetup";

export type CheckId = "agents" | "git" | "github";
export const CHECKS: CheckId[] = ["agents", "git", "github"];

export type CheckState =
  /** Not reached yet. */
  | "pending"
  /** The row the sequence is on, with no answer shown yet (or work in flight:
   *  a git download, the GitHub device flow). */
  | "checking"
  | "ok"
  /** Needs the user: install, retry, or decide. The sequence waits here. */
  | "attention"
  /** The user declined this one; the sequence moved on. */
  | "skipped";

/** The agents panel probes one tile at a time while its row is checking:
 *  this long per tile, then a tail before the row resolves. The row's dwell
 *  is derived from the same numbers so the two always line up. */
export const SCAN_STEP_MS = 260;
export const SCAN_TAIL_MS = 500;
/** How long the git row shows "checking" at minimum. */
const GIT_DWELL_MS = 1100;
/** The GitHub row is a question, not a check: a short beat before it asks. */
const GITHUB_DWELL_MS = 450;
/** How long a green row is seen before the next one starts. */
const ADVANCE_MS = 700;

export interface Preflight {
  /** The row the sequence is on. */
  active: number;
  /** The row the panel on the right shows: a row the user clicked to look
   *  back at, else the active one. */
  stage: number;
  states: Record<CheckId, CheckState>;
  /** Every row has its answer (passed or declined): the sequence is over and
   *  "Open Fletch" may be offered. */
  done: boolean;
  inspect: (index: number) => void;
  skip: (id: CheckId) => void;
}

/** Both automatic checks have an answer, whatever it is. What the machine
 *  looks like, independent of where the animation is. */
export function preflightSettled(setup: OnboardingSetup): boolean {
  return (
    setup.providersProbed && setup.git !== null && !setup.gitDownloading && !setup.installingGit
  );
}

export function usePreflight(setup: OnboardingSetup, githubBusy: boolean): Preflight {
  const [active, setActive] = useState(0);
  const [dwelled, setDwelled] = useState(false);
  const [focus, setFocus] = useState<number | null>(null);
  const [skipped, setSkipped] = useState<Partial<Record<CheckId, boolean>>>({});

  const dwell = [SCAN_STEP_MS * setup.agents.length + SCAN_TAIL_MS, GIT_DWELL_MS, GITHUB_DWELL_MS][
    active
  ];

  // A new row starts its dwell and takes the panel back from any inspection.
  useEffect(() => {
    setDwelled(false);
    setFocus(null);
    const t = window.setTimeout(() => setDwelled(true), dwell);
    return () => window.clearTimeout(t);
  }, [active, dwell]);

  // Per row: do we have an answer, and is it a pass?
  const resolved = [
    setup.providersProbed,
    setup.git !== null && !setup.gitDownloading && !setup.installingGit,
    !githubBusy,
  ];
  const passed = [setup.hasAgent, setup.gitReady, setup.ghConnected];

  // Advance past a row once it is seen green (or skipped). A row needing
  // attention holds the sequence until a skip or a fix flips it.
  const settledRow = resolved[active] && dwelled;
  const clear = passed[active] || !!skipped[CHECKS[active]];
  useEffect(() => {
    if (active >= CHECKS.length - 1 || !settledRow || !clear) return;
    const t = window.setTimeout(() => setActive((a) => Math.max(a, active + 1)), ADVANCE_MS);
    return () => window.clearTimeout(t);
  }, [active, settledRow, clear]);

  const states = {} as Record<CheckId, CheckState>;
  CHECKS.forEach((id, i) => {
    if (i > active) states[id] = "pending";
    else if (i === active && !(resolved[i] && dwelled)) states[id] = "checking";
    else if (passed[i]) states[id] = "ok";
    else if (skipped[id]) states[id] = "skipped";
    else states[id] = "attention";
  });
  const done = CHECKS.every((id) => states[id] === "ok" || states[id] === "skipped");

  const inspect = useCallback(
    (index: number) => {
      if (index > active) return;
      setFocus(index === active ? null : index);
    },
    [active],
  );
  const skip = useCallback((id: CheckId) => setSkipped((s) => ({ ...s, [id]: true })), []);

  return {
    active,
    stage: focus ?? active,
    states,
    done,
    inspect,
    skip,
  };
}
