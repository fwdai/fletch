// The transcript as the user reads it, derived from the flat item list:
//
//   - every run of steps (tool calls, thinking) becomes one row, a `StepRun`,
//     whose header says what the run did;
//   - every turn but the latest folds the work between its prompt and its
//     answer into one row, a `Fold` ("Worked for 4m 12s · changed 9 files"),
//     leaving the prompt and the answer on the page.
//
// The latest turn is never folded: it's the one being watched, or just read.
// It folds when the next one starts, after the user has sent a message and the
// log has pinned to the bottom, so nothing collapses under their eyes.
//
// Pure. Rows refer to items by index into the list, so the per-index
// bookkeeping (live turn, turn ids, footers) carries over unchanged.

import { APP_ACTION_PREFIX } from "@/delegation";
import type { ViewItem } from "./pair";
import { isStep } from "./steps";

/** A run of consecutive steps, rendered as one collapsible row. */
export interface StepRun {
  key: string;
  /** Index of the run's first item. */
  start: number;
  items: ViewItem[];
}

/** The work of a settled turn — everything between its prompt and its
 *  answer — folded behind one row. */
export interface Fold {
  key: string;
  /** Index of the turn's prompt. */
  head: number;
  /** The folded items, `start` (the item after the prompt) up to `end`. */
  start: number;
  end: number;
  items: ViewItem[];
  /** The folded rows, as they render once opened. */
  rows: Row[];
  /** How long the turn ran, when its prompt carries timing. */
  runSec: number | undefined;
}

export type Row =
  | { kind: "item"; index: number }
  | { kind: "steps"; run: StepRun }
  | { kind: "fold"; fold: Fold };

/** Runs and folds by key from the previous pass, reused while their items are
 *  unchanged so memoized rows skip re-rendering during streaming (the same
 *  idea as pair.ts's PairCache, one level up). */
export type RowCache = Map<string, StepRun | Fold>;

export interface TranscriptRowsModel {
  rows: Row[];
  /** Footer indices whose turn's fold already says how long it ran; the
   *  footer leaves the duration out rather than repeating it. */
  quietFooters: Set<number>;
}

function sameItems(a: ViewItem[], b: ViewItem[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i += 1) if (a[i] !== b[i]) return false;
  return true;
}

/** The last index a row covers — where a turn footer that closes it sits. */
export function lastIndex(row: Row): number {
  switch (row.kind) {
    case "item":
      return row.index;
    case "steps":
      return row.run.start + row.run.items.length - 1;
    case "fold":
      return row.fold.end - 1;
  }
}

/** What a settled turn ends on and keeps on the page: the agent's closing
 *  words, and any notice after them (an error the turn ended on, a background
 *  task reporting in). */
function isAnswer(item: ViewItem): boolean {
  return item.kind === "agent_message" || (item.kind === "notice" && !isStep(item));
}

export function buildRows(items: ViewItem[], cache: RowCache): TranscriptRowsModel {
  const seen = new Set<string>();

  const run = (start: number, end: number): StepRun => {
    const key = `s:${start}`;
    seen.add(key);
    const slice = items.slice(start, end);
    const cached = cache.get(key);
    if (cached && "items" in cached && !("rows" in cached) && sameItems(cached.items, slice)) {
      return cached;
    }
    const fresh: StepRun = { key, start, items: slice };
    cache.set(key, fresh);
    return fresh;
  };

  /** Rows for `[start, end)`, steps gathered into runs. */
  const flat = (start: number, end: number): Row[] => {
    const out: Row[] = [];
    let i = start;
    while (i < end) {
      if (!isStep(items[i])) {
        out.push({ kind: "item", index: i });
        i += 1;
        continue;
      }
      let j = i + 1;
      while (j < end && isStep(items[j])) j += 1;
      out.push({ kind: "steps", run: run(i, j) });
      i = j;
    }
    return out;
  };

  /** Fold the turn `[head, end)`, or null when there's nothing worth folding:
   *  no steps at all, or work that already renders as a single row. */
  const fold = (head: number, end: number): { fold: Fold; answerAt: number } | null => {
    let answerAt = end;
    while (answerAt - 1 > head && isAnswer(items[answerAt - 1])) answerAt -= 1;
    const start = head + 1;
    if (answerAt - start < 1) return null;
    const work = items.slice(start, answerAt);
    if (!work.some(isStep)) return null;
    const rows = flat(start, answerAt);
    if (rows.length < 2) return null;

    const prompt = items[head];
    const runSec =
      prompt.kind === "user_message" && prompt.startedAt != null && prompt.endedAt != null
        ? (prompt.endedAt - prompt.startedAt) / 1000
        : undefined;
    const key = `f:${head}`;
    seen.add(key);
    const cached = cache.get(key);
    if (
      cached &&
      "rows" in cached &&
      cached.end === answerAt &&
      cached.runSec === runSec &&
      sameItems(cached.items, work)
    ) {
      return { fold: cached, answerAt };
    }
    const fresh: Fold = { key, head, start, end: answerAt, items: work, rows, runSec };
    cache.set(key, fresh);
    return { fold: fresh, answerAt };
  };

  // Turns start at each prompt; anything before the first (a forked history's
  // inherited tail, a sub-agent thread's whole log) has no prompt to fold under.
  const heads: number[] = [];
  items.forEach((it, i) => {
    if (it.kind === "user_message") heads.push(i);
  });

  const rows: Row[] = [];
  const quietFooters = new Set<number>();
  rows.push(...flat(0, heads.length > 0 ? heads[0] : items.length));
  heads.forEach((head, k) => {
    const end = k + 1 < heads.length ? heads[k + 1] : items.length;
    const latest = k === heads.length - 1;
    const folded = latest ? null : fold(head, end);
    if (!folded) {
      rows.push({ kind: "item", index: head }, ...flat(head + 1, end));
      return;
    }
    rows.push({ kind: "item", index: head }, { kind: "fold", fold: folded.fold });
    rows.push(...flat(folded.answerAt, end));
    // The footer closing a turn says how long it ran; the fold says it first.
    // An app action's turn is footed with its prompt's span (useTranscript),
    // a different duration, so its footer keeps its own.
    const prompt = items[head];
    if (
      folded.fold.runSec !== undefined &&
      prompt.kind === "user_message" &&
      !prompt.text.startsWith(APP_ACTION_PREFIX)
    ) {
      quietFooters.add(end - 1);
    }
  });

  for (const key of cache.keys()) if (!seen.has(key)) cache.delete(key);
  return { rows, quietFooters };
}
