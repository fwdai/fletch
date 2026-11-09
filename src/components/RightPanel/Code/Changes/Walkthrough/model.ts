// The walkthrough's steps, from either source, and where the reader is in them.
// Pure: the hook hands it the tour or the transcript plus the changed files in
// view, and every step it returns points at those files by their
// checkout-relative path — the form `onGo` and the diff pane speak.

import type { FileStatus, Tour } from "@/api";
import type { ViewItem } from "@/components/Workspace/messages/pair";
import {
  stepFacts,
  type ToolCall,
  type ToolResult,
} from "@/components/Workspace/messages/presenters";
import { isPrompt } from "@/components/Workspace/messages/rows";
import { basename } from "@/util/format";
import type { WalkthroughRange } from ".";

export interface StepAnchor {
  path: string;
  range?: WalkthroughRange;
}

export interface Step {
  title: string | null;
  note: string;
  anchors: StepAnchor[];
  /** The step points at files and none is among the changes in view: kept so
   *  its note still reads in order, but there is nothing to show for it. A
   *  step with no anchors (an agent explaining before it points) isn't stale. */
  stale: boolean;
  /** Ordinal of the user turn the step was made in (transcript steps). */
  turn?: number;
}

/** Narration longer than this is cut: the strip is a caption, not the chat. */
const NOTE_MAX = 280;

/** The checkout-relative path a tool's `path` names among `files`: itself, or
 *  the longest file path it ends with — tools often report absolute paths, and
 *  a multi-repo checkout prefixes its files with the repo's subdir. */
export function matchFile(path: string, files: FileStatus[]): string | null {
  let best: string | null = null;
  for (const { path: rel } of files) {
    if (path !== rel && !path.endsWith(`/${rel}`)) continue;
    if (best === null || rel.length > best.length) best = rel;
  }
  return best;
}

/** The step's first anchor among the changes in view — where going to the
 *  step lands. Undefined for a stale step. */
export function presentAnchor(step: Step, files: FileStatus[]): StepAnchor | undefined {
  return step.anchors.find((a) => files.some((f) => f.path === a.path));
}

/** The step's files among the changes in view, each once — what reading the
 *  step counts as having viewed. */
export function presentPaths(step: Step, files: FileStatus[]): string[] {
  const paths = step.anchors.map((a) => a.path).filter((p) => files.some((f) => f.path === p));
  return [...new Set(paths)];
}

export function stepsFromTour(tour: Tour, files: FileStatus[]): Step[] {
  return tour.steps.map((s) => {
    // Anchors already name files the way the list does (the host enforces
    // relative paths), so an anchor is kept as given and simply absent from
    // the list when its file is not in view.
    const step: Step = {
      title: s.title,
      note: s.note,
      anchors: s.anchors.map((a) => ({ path: a.path, range: { start: a.start, end: a.end } })),
      stale: false,
    };
    step.stale = step.anchors.length > 0 && presentAnchor(step, files) === undefined;
    return step;
  });
}

interface Draft {
  /** Normalised where the path matches a file, as the tool gave it otherwise. */
  paths: string[];
  note: string;
  turn?: number;
}

/** Steps as the transcript tells them: files in the order the agent edited
 *  them, each step with the prose the agent wrote just before. An edit only to
 *  files the current step already covers extends it, so a file edited five
 *  times in a row is one step; a user prompt ends the step, so a step never
 *  spans two turns. Narration belongs to the edit right after it: a step takes
 *  it, an extending edit consumes it (adopting it if the step had none), so a
 *  later edit the agent didn't narrate isn't captioned with stale prose. */
export function deriveStepsFromTranscript(items: ViewItem[], files: FileStatus[]): Step[] {
  const drafts: Draft[] = [];
  /** Whether the last draft still takes edits to its own files. */
  let open = false;
  let pending: string | null = null;
  let turn: number | undefined;
  let prompts = 0;

  for (const item of items) {
    if (isPrompt(item)) {
      turn = prompts;
      prompts += 1;
      open = false;
      pending = null;
      continue;
    }
    if (item.kind === "agent_message") {
      if (!item.streaming && item.text.trim()) pending = item.text;
      continue;
    }
    const paths = changedPaths(item, files);
    if (paths.length === 0) continue;
    const narration = pending ? noteFrom(pending) : "";
    pending = null;
    const last = drafts[drafts.length - 1];
    if (open && paths.every((p) => last.paths.includes(p))) {
      last.note ||= narration;
      continue;
    }
    drafts.push({ paths, note: narration, turn });
    open = true;
  }

  return drafts.flatMap((d) => {
    const anchors = d.paths
      .filter((p) => files.some((f) => f.path === p))
      .map((path) => ({ path }));
    if (anchors.length === 0) return [];
    const note = d.note || editedNote(anchors);
    return [{ title: null, note, anchors, stale: false, turn: d.turn }];
  });
}

/** Raw paths per tool call. A call keeps its identity across renders, and the
 *  transcript is re-derived on every streamed token — while an Edit's facts
 *  run a line diff of its old and new text. */
const callPaths = new WeakMap<ToolCall, string[]>();

/** The files a settled, successful edit touched, matched to `files` where they
 *  match, each once. Empty for anything that isn't an edit. */
function changedPaths(item: ViewItem, files: FileStatus[]): string[] {
  let call: ToolCall;
  let result: ToolResult | null = null;
  if (item.kind === "tool_pair") {
    call = item.call;
    result = item.result;
  } else if (item.kind === "tool_call") {
    call = item;
  } else {
    return [];
  }
  // A streaming call's input is still partial; a failed edit changed nothing.
  if (call.streaming || result?.is_error) return [];
  let raw = callPaths.get(call);
  if (!raw) {
    const facts = stepFacts(call, result);
    // `path` is "" when an edit names no file; a patch names its files in `paths`.
    raw = facts.kind === "change" ? [...(facts.paths ?? []), facts.path ?? ""].filter(Boolean) : [];
    callPaths.set(call, raw);
  }
  return [...new Set(raw.map((p) => matchFile(p, files) ?? p))];
}

function editedNote(anchors: StepAnchor[]): string {
  return anchors.length === 1
    ? `Edited ${basename(anchors[0].path)}`
    : `Edited ${anchors.length} files`;
}

const FENCED = /(```|~~~)[\s\S]*?(?:\1|$)/g;
const HEADING = /^\s*#{1,6}(?:\s|$)/;
const BULLET = /^\s*(?:[-*+]|\d+[.)])\s+/;

/** A message reduced to a caption: its first paragraph of prose — code blocks
 *  and headings dropped, list markers stripped, whitespace folded — cut to
 *  `NOTE_MAX` on a word boundary. Empty when the message has no prose. */
export function noteFrom(text: string): string {
  for (const para of text.replace(FENCED, "\n\n").split(/\n\s*\n/)) {
    const prose = para
      .split("\n")
      .filter((line) => !HEADING.test(line))
      .map((line) => line.replace(BULLET, ""))
      .join(" ")
      .replace(/\s+/g, " ")
      .trim();
    if (prose) return clip(prose, NOTE_MAX);
  }
  return "";
}

function clip(text: string, max: number): string {
  if (text.length <= max) return text;
  const head = text.slice(0, max - 1);
  const space = head.lastIndexOf(" ");
  // Cut back to the last whole word, unless the cut already falls between two.
  const words = /\s/.test(text[max - 1]) || space <= 0 ? head : head.slice(0, space);
  return `${words.trimEnd()}…`;
}

/** The step the reader is on while the diff pane shows `currentPath`: the one
 *  whose anchors include it, the nearest to `lastIndex` when several do (the
 *  later one on a tie — reading moves forward). -1 when none does. */
export function stepIndexFor(steps: Step[], currentPath: string | null, lastIndex: number): number {
  if (currentPath === null) return -1;
  let best = -1;
  steps.forEach((step, i) => {
    if (!step.anchors.some((a) => a.path === currentPath)) return;
    if (best === -1 || Math.abs(i - lastIndex) <= Math.abs(best - lastIndex)) best = i;
  });
  return best;
}
