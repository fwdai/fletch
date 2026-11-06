// Where a roadmap code goes when something asks to be shown it.
//
// Codes are addresses. The PM quotes them in chat, the standup digest quotes them,
// a run's monitor carries one, and every one of those is rendered as something to
// click. Until now "click" meant one thing — focus the row on the board — and the
// board does not render every row: a shipped item leaves it entirely. So a chip
// for a done item was a dead click, and the standup digest, whose whole subject is
// what shipped, manufactured them by the handful.
//
// The fix is to make the destination a decision rather than an assumption, in one
// place, so every caller resolves it the same way and a code with nowhere to go is
// a refusal the user can read instead of a click that does nothing.

/** What is on the other end of a code. */
export type RevealTarget =
  /** A row the board draws: focus it there. */
  | { kind: "board"; code: string }
  /** A shipped row. It left the board, and where it lives now is the Activity
   *  tab's record of what has been built. */
  | { kind: "shipped"; code: string }
  /** No such code on this project's board. A typo, another project's prefix, or a
   *  code whose row was discarded since it was quoted — the PM's transcript is
   *  permanent and the board is not. */
  | { kind: "unknown"; code: string };

/** Resolve a code against the board's rows.
 *
 *  Takes the *whole* row buffer, `done` items included, because "it shipped" and
 *  "there is no such thing" are different answers and only the full buffer can
 *  tell them apart. Callers that hold the filtered board set cannot ask this
 *  question. */
export function revealTarget(
  code: string,
  rows: readonly { code: string; status: string }[],
): RevealTarget {
  const row = rows.find((r) => r.code === code);
  if (!row) return { kind: "unknown", code };
  return { kind: row.status === "done" ? "shipped" : "board", code };
}

/** Why a reveal couldn't happen, for the board's error bar. Only `unknown` has
 *  nothing to say for itself — the other two land somewhere. */
export function revealRefusal(target: RevealTarget): string | null {
  return target.kind === "unknown"
    ? `${target.code} isn't on this project's roadmap — it may have been discarded.`
    : null;
}
