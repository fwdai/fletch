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
// bookkeeping (live turn, turn ids, footers) carries over unchanged. Their
// keys, though, follow the items (`itemIdentity`): a run is named by the first
// tool call in it and a fold by its prompt, so the log can be rebuilt at turn
// end or grow a page of older history at the top (the phone's "Load older
// messages") without a run's disclosure state moving to a different run.

import { APP_ACTION_PREFIX } from "@/delegation";
import { itemIdentity, type ViewItem } from "./pair";
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
export interface RowCache {
  runs: Map<string, StepRun>;
  folds: Map<string, Fold>;
}

export const newRowCache = (): RowCache => ({ runs: new Map(), folds: new Map() });

export interface TranscriptRowsModel {
  rows: Row[];
  /** Footer indices whose turn's fold already says how long it ran; the
   *  footer leaves the duration out rather than repeating it. */
  quietFooters: Set<number>;
  /** Key of the run the transcript ends on — a mid-turn follow-up bubble
   *  below it aside — which is the one the agent is on while the turn is
   *  live. Null when it ends on something else. */
  tailRunKey: string | null;
}

/** Whether the log ends on a step — a follow-up bubble queued below it aside
 *  — so a run's own live line is on the page and no second "working"
 *  indicator is needed under it. */
export function endsInSteps(items: ViewItem[]): boolean {
  for (let i = items.length - 1; i >= 0; i -= 1) {
    if (items[i].kind === "queued_message") continue;
    return isStep(items[i]);
  }
  return false;
}

/** A prompt the user wrote. An app action (`[app-action] …`, the chip a git
 *  button sends) is a message too, but not a turn of its own in the user's
 *  eyes: it continues the turn it was clicked in. */
export function isPrompt(item: ViewItem): item is Extract<ViewItem, { kind: "user_message" }> {
  return item.kind === "user_message" && !item.text.startsWith(APP_ACTION_PREFIX);
}

/** A turn as the user counts them: `[head, end)` from a prompt they wrote up
 *  to the next, app actions and their replies inside it. One model for every
 *  per-turn derivation — the footer that closes a turn (`useTranscript`), the
 *  navigable turns, and which footer a fold has already spoken for — so none
 *  of them can place a turn's end somewhere another doesn't. */
export interface TurnSpan {
  head: number;
  end: number;
}

export function turnSpans(items: ViewItem[]): TurnSpan[] {
  const spans: TurnSpan[] = [];
  items.forEach((it, i) => {
    if (!isPrompt(it)) return;
    if (spans.length > 0) spans[spans.length - 1].end = i;
    spans.push({ head: i, end: items.length });
  });
  return spans;
}

/** Index where the latest turn begins: its prompt, or 0 before any. Only rows
 *  at or after it may show a live spinner, so a dangling call from an
 *  interrupted earlier turn can't light up when a later turn goes busy. */
export function openTurnStart(items: ViewItem[]): number {
  for (let i = items.length - 1; i >= 0; i -= 1) {
    if (items[i].kind === "user_message") return i;
  }
  return 0;
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

  /** A run's name: its first tool call, else its first identified item, else
   *  its position. */
  const runKey = (start: number, end: number): string => {
    for (let i = start; i < end; i += 1) {
      const it = items[i];
      if (it.kind === "tool_pair" || it.kind === "tool_call") {
        const id = itemIdentity(it);
        if (id) return `s:${id}`;
      }
    }
    for (let i = start; i < end; i += 1) {
      const id = itemIdentity(items[i]);
      if (id) return `s:${id}`;
    }
    return `s:i:${start}`;
  };

  const run = (start: number, end: number): StepRun => {
    const key = runKey(start, end);
    seen.add(key);
    const slice = items.slice(start, end);
    // Same key and items at the same place: the object is reused. The same
    // items at a new place (older history prepended above them) get a fresh
    // object under the same key — the key keeps the row's state, the object
    // carries the indices the footers and the live turn are placed by.
    const cached = cache.runs.get(key);
    if (cached && cached.start === start && sameItems(cached.items, slice)) return cached;
    const fresh: StepRun = { key, start, items: slice };
    cache.runs.set(key, fresh);
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
    const key = `f:${itemIdentity(prompt) ?? `i:${head}`}`;
    seen.add(key);
    const cached = cache.folds.get(key);
    if (
      cached &&
      cached.head === head &&
      cached.end === answerAt &&
      cached.runSec === runSec &&
      sameItems(cached.items, work)
    ) {
      return { fold: cached, answerAt };
    }
    const fresh: Fold = { key, head, start, end: answerAt, items: work, rows, runSec };
    cache.folds.set(key, fresh);
    return { fold: fresh, answerAt };
  };

  // Work folds under the message that asked for it — a prompt, or an app
  // action clicked after the reply, whose own work is its own fold. Anything
  // before the first message (a forked history's inherited tail, a sub-agent
  // thread's whole log) has nothing to fold under.
  const heads: number[] = [];
  items.forEach((it, i) => {
    if (it.kind === "user_message") heads.push(i);
  });

  const rows: Row[] = [];
  /** How long each folded message's work ran, by its index. */
  const spoken = new Map<number, number>();
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
    if (folded.fold.runSec !== undefined) spoken.set(head, folded.fold.runSec);
  });

  // The footer closing a turn says how long its prompt ran; when the fold
  // under that prompt said it first, the footer leaves it out. The footer
  // sits where the turn ends as the user counts turns (`turnSpans`): after an
  // app action's reply, not before the action.
  const quietFooters = new Set<number>();
  for (const span of turnSpans(items)) {
    if (spoken.has(span.head)) quietFooters.add(span.end - 1);
  }

  for (const key of cache.runs.keys()) if (!seen.has(key)) cache.runs.delete(key);
  for (const key of cache.folds.keys()) if (!seen.has(key)) cache.folds.delete(key);

  let tailRunKey: string | null = null;
  for (let r = rows.length - 1; r >= 0; r -= 1) {
    const row = rows[r];
    if (row.kind === "item" && items[row.index].kind === "queued_message") continue;
    tailRunKey = row.kind === "steps" ? row.run.key : null;
    break;
  }
  return { rows, quietFooters, tailRunKey };
}
