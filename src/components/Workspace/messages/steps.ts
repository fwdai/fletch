// A run of steps — the agent working (tool calls, thinking) rather than talking
// to the user — said in one line: what it changed, ran and looked at. Plus the
// present-tense line for a run still in flight. Pure; the rendering is
// StepGroup's.

import { isSubagentCall } from "@/adapters/shared/subagents";
import type { IconName } from "@/components/Icon";
import { basename } from "@/util/format";
import { renderToolResult } from "@/util/toolText";
import type { ViewItem } from "./pair";
import { getPresenter, type StepKind, stepActivity, stepFacts, type ToolCall } from "./presenters";
import { isUserInputTool } from "./UserInput/parse";

/** Whether `item` folds into a run of steps. Questions for the user and
 *  sub-agent launches never do — one is waiting on the user, the other is a
 *  thread of its own — and neither does anything the agent says. */
export function isStep(item: ViewItem): boolean {
  switch (item.kind) {
    case "tool_pair":
      return !isUserInputTool(item.call.name) && !isSubagentCall(item.call);
    case "tool_call":
      return !isUserInputTool(item.name) && !isSubagentCall(item);
    case "tool_result":
      return true;
    case "notice":
      return item.subtype === "reasoning" || item.subtype === "hook_output";
    default:
      return false;
  }
}

/** What a run of steps (or a whole turn's work) did. Files are counted once
 *  however many times they were touched; everything else is counted by call. */
export interface StepSummary {
  /** Tool calls, sub-agent launches included. */
  calls: number;
  thoughts: number;
  changed: { paths: string[]; additions: number; deletions: number };
  ran: string[];
  read: string[];
  searched: string[];
  planned: number;
  other: string[];
  subagents: number;
  /** Calls whose result was an error, and error notices. */
  failed: number;
}

function callOf(item: ViewItem): { call: ToolCall; failed: boolean } | null {
  if (item.kind === "tool_pair") return { call: item.call, failed: !!item.result?.is_error };
  if (item.kind === "tool_call") return { call: item, failed: false };
  return null;
}

export function summarizeSteps(items: ViewItem[]): StepSummary {
  const s: StepSummary = {
    calls: 0,
    thoughts: 0,
    changed: { paths: [], additions: 0, deletions: 0 },
    ran: [],
    read: [],
    searched: [],
    planned: 0,
    other: [],
    subagents: 0,
    failed: 0,
  };
  for (const item of items) {
    if (item.kind === "notice") {
      if (item.subtype === "reasoning") s.thoughts += 1;
      else if (item.is_error || item.subtype === "error") s.failed += 1;
      continue;
    }
    const c = callOf(item);
    if (!c) continue;
    s.calls += 1;
    if (c.failed) s.failed += 1;
    if (isSubagentCall(c.call)) {
      s.subagents += 1;
      continue;
    }
    if (isUserInputTool(c.call.name)) continue;
    const facts = stepFacts(c.call, item.kind === "tool_pair" ? item.result : null);
    switch (facts.kind) {
      case "change":
        if (facts.path && !s.changed.paths.includes(facts.path)) s.changed.paths.push(facts.path);
        s.changed.additions += facts.additions ?? 0;
        s.changed.deletions += facts.deletions ?? 0;
        break;
      case "read":
        if (facts.path && !s.read.includes(facts.path)) s.read.push(facts.path);
        break;
      case "run":
        s.ran.push(facts.subject ?? "");
        break;
      case "search":
        s.searched.push(facts.subject ?? "");
        break;
      case "plan":
        s.planned += 1;
        break;
      default:
        if (facts.subject && !s.other.includes(facts.subject)) s.other.push(facts.subject);
        else if (!facts.subject) s.other.push("tool");
    }
  }
  return s;
}

/** One phrase of the summary line. `text` is lower-case; the line capitalizes
 *  whichever phrase leads. `code` is a name shown in mono after the text. */
export interface SummaryPart {
  key: string;
  text: string;
  code?: string;
  diff?: { additions: number; deletions: number };
  tone?: "danger";
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** Two names are worth spelling out; past that a count reads faster. */
function named(paths: string[], noun: string): string {
  return paths.length <= 2 ? paths.map(basename).join(", ") : plural(paths.length, noun);
}

function clip(text: string, max: number): string {
  const one = text.replace(/\s+/g, " ").trim();
  return one.length > max ? `${one.slice(0, max - 1)}…` : one;
}

/** The summary as phrases, most consequential first: what changed on disk,
 *  then what ran, then what was looked at. Failures go last, in red — the eye
 *  finds them there, and a recovered failure doesn't lead the line. */
export function summaryParts(s: StepSummary): SummaryPart[] {
  const parts: SummaryPart[] = [];
  if (s.changed.paths.length > 0) {
    parts.push({
      key: "changed",
      text: `changed ${named(s.changed.paths, "file")}`,
      diff: { additions: s.changed.additions, deletions: s.changed.deletions },
    });
  }
  if (s.subagents > 0) {
    parts.push({ key: "subagents", text: `delegated to ${plural(s.subagents, "sub-agent")}` });
  }
  if (s.ran.length === 1 && s.ran[0]) {
    parts.push({ key: "ran", text: "ran", code: clip(s.ran[0], 40) });
  } else if (s.ran.length > 0) {
    parts.push({ key: "ran", text: `ran ${plural(s.ran.length, "command")}` });
  }
  if (s.read.length > 0) parts.push({ key: "read", text: `read ${named(s.read, "file")}` });
  if (s.searched.length === 1 && s.searched[0]) {
    parts.push({ key: "searched", text: "searched for", code: clip(s.searched[0], 32) });
  } else if (s.searched.length > 0) {
    parts.push({ key: "searched", text: `searched ${plural(s.searched.length, "time")}` });
  }
  if (s.other.length > 0) {
    parts.push({
      key: "other",
      text: s.other.length <= 2 ? `used ${s.other.join(", ")}` : `used ${s.other.length} tools`,
    });
  }
  if (s.planned > 0) parts.push({ key: "planned", text: "updated the plan" });
  if (parts.length === 0 && s.thoughts > 0) parts.push({ key: "thought", text: "thought" });
  if (s.failed > 0) parts.push({ key: "failed", text: `${s.failed} failed`, tone: "danger" });
  return parts;
}

/** The icon of a step kind, for a summary that leads with it. */
const KIND_ICON: Record<StepKind, IconName> = {
  change: "edit",
  run: "terminal",
  read: "file",
  search: "search",
  plan: "task",
  other: "wrench",
};

/** The glyph a run's header rests on: a lone call's own icon; for a mixed run,
 *  the icon of what leads its summary. */
export function runIcon(items: ViewItem[], summary: StepSummary): IconName {
  const calls = items.flatMap((it) => {
    const c = callOf(it);
    return c ? [c.call] : [];
  });
  if (calls.length === 1) return getPresenter(calls[0].name).icon ?? "wrench";
  if (summary.changed.paths.length > 0) return KIND_ICON.change;
  if (summary.subagents > 0) return "subagent";
  if (summary.ran.length > 0) return KIND_ICON.run;
  if (summary.read.length > 0) return KIND_ICON.read;
  if (summary.searched.length > 0) return KIND_ICON.search;
  if (calls.length > 0) return KIND_ICON.other;
  return "sparkle";
}

/** What a live run is doing right now. `idle` marks the in-between — the last
 *  call came back and nothing new has started — which the header holds back
 *  briefly so it doesn't flash "Thinking" between every pair of calls. */
export interface LiveActivity {
  label: string;
  icon: IconName;
  idle: boolean;
}

const MANY: Record<StepKind, (n: number) => string> = {
  change: (n) => `Editing ${n} files`,
  run: (n) => `Running ${n} commands`,
  read: (n) => `Reading ${n} files`,
  search: (n) => `Running ${n} searches`,
  plan: () => "Updating the plan",
  other: (n) => `Running ${n} tools`,
};

export function liveActivity(items: ViewItem[]): LiveActivity {
  // Calls still out: everything without a result yet. Parallel calls come
  // back in any order, so this is a set, not just the last row.
  const pending: ToolCall[] = [];
  for (const it of items) {
    if (it.kind === "tool_pair" && !it.result) pending.push(it.call);
    else if (it.kind === "tool_call") pending.push(it);
  }
  if (pending.length === 1) {
    const call = pending[0];
    return {
      label: stepActivity(call),
      icon: getPresenter(call.name).icon ?? "wrench",
      idle: false,
    };
  }
  if (pending.length > 1) {
    const kinds = new Set(pending.map((c) => stepFacts(c, null).kind));
    const first = pending[0];
    const icon = getPresenter(first.name).icon ?? "wrench";
    if (kinds.size === 1) {
      const [kind] = kinds;
      return { label: MANY[kind](pending.length), icon, idle: false };
    }
    return { label: `${stepActivity(first)} and ${pending.length - 1} more`, icon, idle: false };
  }
  const last = items[items.length - 1];
  const thinking = last?.kind === "notice" && last.subtype === "reasoning";
  return { label: "Thinking", icon: "sparkle", idle: !thinking };
}

/** Text of an item a find-in-conversation query can match: what the row shows
 *  collapsed and what it shows opened, lower-cased. Cached per item — the log
 *  is immutable and a query runs this over every row on each keystroke. */
const SEARCH_TEXT = new WeakMap<object, string>();

function valuesOf(value: unknown, out: string[]): void {
  if (value == null) return;
  if (typeof value === "string") out.push(value);
  else if (typeof value === "number" || typeof value === "boolean") out.push(String(value));
  else if (Array.isArray(value)) for (const v of value) valuesOf(v, out);
  else if (typeof value === "object") for (const v of Object.values(value)) valuesOf(v, out);
}

export function searchText(item: ViewItem): string {
  const hit = SEARCH_TEXT.get(item);
  if (hit !== undefined) return hit;
  const out: string[] = [];
  switch (item.kind) {
    case "tool_pair":
      out.push(item.call.name);
      valuesOf(item.call.input, out);
      if (item.result) out.push(renderToolResult(item.result.content));
      break;
    case "tool_call":
      out.push(item.name);
      valuesOf(item.input, out);
      break;
    case "tool_result":
      out.push(renderToolResult(item.content));
      break;
    default:
      out.push(item.text);
  }
  const text = out.join("\n").toLowerCase();
  SEARCH_TEXT.set(item, text);
  return text;
}

/** The shortest query worth opening folded rows for. One or two letters match
 *  nearly every tool call, and opening all of them would bury the matches. */
export const REVEAL_MIN_QUERY = 3;
