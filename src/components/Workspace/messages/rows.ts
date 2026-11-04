// The transcript as the user reads it, derived from the flat item list: every
// run of steps (tool calls, thinking) becomes one row, a `StepRun`, whose header
// says what the run did, and everything else is a row of its own, on the page.
//
// `isStep` is the only rule, the same in every turn, live or settled. What the
// agent says and the sub-agents it starts are never collapsed, wherever they
// sit in a turn, so nothing here has to tell an answer from narration: a turn
// can end on a tool call, and a scheduled wake-up can add to it long after.
//
// Pure. Rows refer to items by index into the list, so the per-index
// bookkeeping (live turn, turn ids, footers) carries over unchanged. A run's
// key, though, follows its items (`itemIdentity`): it is named by the first
// tool call in it, so the log can be rebuilt at turn end or grow a page of
// older history at the top (the phone's "Load older messages") without a run's
// disclosure state moving to a different run.

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

export type Row = { kind: "item"; index: number } | { kind: "steps"; run: StepRun };

/** Runs by key from the previous pass, reused while their items are unchanged
 *  so memoized rows skip re-rendering during streaming (the same idea as
 *  pair.ts's PairCache, one level up). */
export type RowCache = Map<string, StepRun>;

export const newRowCache = (): RowCache => new Map();

export interface TranscriptRowsModel {
  rows: Row[];
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
 *  per-turn derivation — the footer that closes a turn and the navigable
 *  turns — so none of them can place a turn's end somewhere another doesn't. */
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
  return row.kind === "item" ? row.index : row.run.start + row.run.items.length - 1;
}

/** A turn's closing footer: how long it ran, its settled prose (for copy), and
 *  the turn a fork from here continues through — the last turn above the
 *  footer, which is an app action when one closed the span. */
export interface TurnFooterData {
  runSec: number;
  copyText: string;
  forkTurnId: string | undefined;
}

export interface TurnFooters {
  /** Parallel to the items: a footer at each ended turn's last item. */
  footers: (TurnFooterData | null)[];
  /** Persisted start of the turn still running, if one is. */
  openStartedAt: number | undefined;
}

/** The footer closing each turn, placed at its last item: the seam before the
 *  next turn, as `turnSpans` counts turns. A turn ran for as long as every
 *  message in it did (its prompt, and any app action clicked in it), each with
 *  the runs the provider opened on its own after it. A message still running
 *  keeps the turn open, and an open turn carries no footer, just its live
 *  timer on the working strip. */
export function turnFooters(items: ViewItem[]): TurnFooters {
  const footers: (TurnFooterData | null)[] = items.map(() => null);
  let openStartedAt: number | undefined;
  for (const { head, end } of turnSpans(items)) {
    let runMs: number | undefined;
    let open: number | undefined;
    let forkTurnId: string | undefined;
    const texts: string[] = [];
    for (let j = head; j < end; j += 1) {
      const it = items[j];
      if (it.kind === "agent_message" && !it.streaming && it.text) texts.push(it.text);
      if (it.kind !== "user_message") continue;
      if (it.turnId) forkTurnId = it.turnId;
      if (it.startedAt == null) continue;
      if (it.endedAt == null) open ??= it.startedAt;
      else runMs = (runMs ?? 0) + it.endedAt - it.startedAt + (it.providerRunMs ?? 0);
    }
    if (open !== undefined) openStartedAt = open;
    else if (runMs !== undefined) {
      footers[end - 1] = { runSec: runMs / 1000, copyText: texts.join("\n\n"), forkTurnId };
    }
  }
  return { footers, openStartedAt };
}

/** The footer closing `row`, if one does. The turn the agent is on shows none
 *  yet, even when its messages have all ended: a run the provider opened on
 *  its own after them is that turn's work too, and still adding up. */
export function footerFor(
  row: Row,
  footers: (TurnFooterData | null)[],
  liveBusy: boolean,
  openStart: number,
): TurnFooterData | null {
  const end = lastIndex(row);
  return liveBusy && end >= openStart ? null : footers[end];
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
    const cached = cache.get(key);
    if (cached && cached.start === start && sameItems(cached.items, slice)) return cached;
    const fresh: StepRun = { key, start, items: slice };
    cache.set(key, fresh);
    return fresh;
  };

  const rows: Row[] = [];
  let i = 0;
  while (i < items.length) {
    if (!isStep(items[i])) {
      rows.push({ kind: "item", index: i });
      i += 1;
      continue;
    }
    let j = i + 1;
    while (j < items.length && isStep(items[j])) j += 1;
    rows.push({ kind: "steps", run: run(i, j) });
    i = j;
  }

  for (const key of cache.keys()) if (!seen.has(key)) cache.delete(key);

  let tailRunKey: string | null = null;
  for (let r = rows.length - 1; r >= 0; r -= 1) {
    const row = rows[r];
    if (row.kind === "item" && items[row.index].kind === "queued_message") continue;
    tailRunKey = row.kind === "steps" ? row.run.key : null;
    break;
  }
  return { rows, tailRunKey };
}
