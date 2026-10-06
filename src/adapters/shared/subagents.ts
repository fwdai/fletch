// Nesting a sub-agent's events under the tool call that spawned it.
//
// Claude's stream-json tags every sidechain envelope with the spawning
// Task/Agent tool_use id (top-level `parent_tool_use_id`), and the transcript
// sync stamps the same field onto the persisted records of every provider that
// writes sub-agents to files of their own (Claude's `subagents/agent-*.jsonl`,
// Codex's child rollouts). Any reducer whose events can carry that tag routes
// through here, so the nesting logic exists once.

import type { ChatItem, RawEvent } from "@/adapters/types";
import type { BackgroundTask } from "./backgroundTasks";

type Reducer = (prev: ChatItem[], ev: RawEvent) => ChatItem[];

export type ToolCall = Extract<ChatItem, { kind: "tool_call" }>;
export type ToolResult = Extract<ChatItem, { kind: "tool_result" }>;

/** The spawning tool_use id an event is tagged with; null for the main agent. */
export function parentToolUseId(ev: RawEvent): string | null {
  const v = ev.parent_tool_use_id;
  return typeof v === "string" && v.length > 0 ? v : null;
}

/** Fold a sub-agent event into the children of the tool_call it belongs to,
 *  reducing it there with the provider's own top-level logic. Searches nested
 *  tool_calls so a sub-agent that itself spawns a sub-agent threads correctly.
 *  If the parent tool_call isn't present yet (ordering race), returns `items`
 *  unchanged — the event is dropped rather than leaked into the main log. */
export function routeToChild(
  items: ChatItem[],
  parentId: string,
  ev: RawEvent,
  reduceTop: Reducer,
): ChatItem[] {
  for (let i = items.length - 1; i >= 0; i -= 1) {
    const it = items[i];
    if (it.kind !== "tool_call") continue;
    if (it.id === parentId) {
      const next = items.slice();
      next[i] = { ...it, children: reduceTop(it.children ?? [], ev) };
      return next;
    }
    if (it.children && it.children.length > 0) {
      const updated = routeToChild(it.children, parentId, ev, reduceTop);
      // routeToChild returns the same array reference when it finds no match,
      // so an identity change means the parent lived inside these children.
      if (updated !== it.children) {
        const next = items.slice();
        next[i] = { ...it, children: updated };
        return next;
      }
    }
  }
  return items;
}

/** A provider's reducer with sub-agent routing in front: tagged events go to
 *  their tool_call's children, everything else to `reduceTop` as before. */
export function withSubagentRouting(reduceTop: Reducer): Reducer {
  return (prev, ev) => {
    const parentId = parentToolUseId(ev);
    return parentId ? routeToChild(prev, parentId, ev, reduceTop) : reduceTop(prev, ev);
  };
}

// ── Reading a thread back out ───────────────────────────────────────────────
//
// A sub-agent thread is addressed by the chain of tool_use ids that launched
// it: the first names a tool_call in the main log, each later one a tool_call
// among the previous call's `children`. Pure and framework-free, so the
// desktop's thread pane and the phone's thread screen resolve the same thread
// the same way.

/** The tool names that launch a sub-agent, lowercased: Claude's Agent (and
 *  its former name Task, which Cursor still uses). */
const SUBAGENT_TOOLS = new Set(["agent", "task"]);

/** A tool call that stands for a sub-agent run: a known spawn tool, or any
 *  call the reducer has threaded sidechain events under (a provider whose
 *  spawn tool goes by another name still gets its children). */
export function isSubagentCall(item: ChatItem): boolean {
  if (item.kind !== "tool_call") return false;
  return SUBAGENT_TOOLS.has(item.name.toLowerCase()) || (item.children?.length ?? 0) > 0;
}

export interface ResolvedThread {
  /** The call that launched this thread (the last one on the path). */
  call: ToolCall;
  /** The thread's own log: the launching call's children. */
  items: ChatItem[];
  /** Every launching call along the path, main log first — the breadcrumb. */
  trail: ToolCall[];
  /** The log `call` sits in (the main log, or the parent thread's items):
   *  where its result and its siblings are. */
  parent: ChatItem[];
  /** Sub-agent calls in `parent`, in launch order. */
  siblings: ToolCall[];
  /** `call`'s position among `siblings`. */
  index: number;
}

/** Walk `path` down from `log`. Null when any id on the path is not a tool
 *  call where it should be — the history is not loaded yet, or a rewind took
 *  the launch away. */
export function resolveThread(log: ChatItem[] | undefined, path: string[]): ResolvedThread | null {
  if (path.length === 0) return null;
  let parent: ChatItem[] = log ?? [];
  const trail: ToolCall[] = [];
  let call: ToolCall | undefined;
  for (const id of path) {
    if (call) parent = call.children ?? [];
    call = parent.find((it): it is ToolCall => it.kind === "tool_call" && it.id === id);
    if (!call) return null;
    trail.push(call);
  }
  if (!call) return null;
  const siblings = parent.filter((it): it is ToolCall => isSubagentCall(it));
  return {
    call,
    items: call.children ?? [],
    trail,
    parent,
    siblings,
    index: siblings.indexOf(call),
  };
}

function inputField(input: unknown, key: string): string {
  if (input && typeof input === "object" && key in input) {
    const v = (input as Record<string, unknown>)[key];
    if (typeof v === "string") return v;
  }
  return "";
}

/** What a thread is called: the launch description, else the sub-agent type,
 *  else a generic label — the same precedence as a sidebar child row. */
export function threadLabel(call: ToolCall): string {
  return (
    inputField(call.input, "description") || inputField(call.input, "subagent_type") || "sub-agent"
  );
}

export function threadType(call: ToolCall): string {
  return inputField(call.input, "subagent_type");
}

/** The thread's result as the sub-agent reported it (its tool_result in the
 *  parent log), or null while it is still running / before it was recorded. */
export function threadResult(parent: ChatItem[], call: ToolCall): ToolResult | null {
  for (const it of parent) {
    if (it.kind === "tool_result" && it.tool_use_id === call.id) return it;
  }
  return null;
}

export type ThreadState = "running" | "failed" | "done";

/** Live state of a thread. `parentBusy` is the launching agent's own liveness:
 *  a foreground sub-agent has no background task, so until its result lands
 *  the parent being mid-turn is the only sign it is still going. A
 *  backgrounded one reports through its task after the launch result ("Async
 *  agent launched…") has already landed, so the task outranks the result. */
export function threadState(
  result: ToolResult | null,
  task: BackgroundTask | undefined,
  parentBusy: boolean,
): ThreadState {
  if (task?.status === "running") return "running";
  if (task?.status === "failed") return "failed";
  if (!result) return parentBusy ? "running" : "done";
  return result.is_error ? "failed" : "done";
}

/** Rows that count as the sub-agent doing something: its prose and its tool
 *  calls. Notices and results ride along with those. */
export function threadSteps(items: ChatItem[]): number {
  let n = 0;
  for (const it of items) if (it.kind === "tool_call" || it.kind === "agent_message") n += 1;
  return n;
}

// ── Which sub-agent opened which PR ─────────────────────────────────────────
//
// The host records a PR against the checkout, not against the thread that
// asked for it, so attribution is read back out of the log: the `open_pr`
// mailbox op answers with the new PR's URL as its stdout, and that answer lands
// as a tool_result inside the sub-agent's thread.

const PR_URL = String.raw`https://github\.com/[^/\s"]+/[^/\s"]+/pull/(\d+)`;
/** Only the shape an `open_pr` answer takes: the mailbox response JSON
 *  (`"stdout":"<url>"`). A bare URL is not enough — `gh pr view` / `gh pr list`
 *  print one too, which would credit a sub-agent that merely looked at a
 *  sibling's PR. */
const OPENED_PR = new RegExp(String.raw`"stdout"\s*:\s*"${PR_URL}`, "g");

/** A tool_result's content as text: a string as-is, Claude's content blocks by
 *  their text, anything else as nothing. */
function resultText(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .map((b) =>
      b && typeof b === "object" && typeof (b as { text?: unknown }).text === "string"
        ? (b as { text: string }).text
        : "",
    )
    .join("\n");
}

/** Results are immutable once logged and the reducer keeps their identity
 *  across updates, so each is scanned once however often the log re-renders. */
const opened = new WeakMap<ToolResult, number[]>();

function openedPrs(result: ToolResult): number[] {
  let found = opened.get(result);
  if (!found) {
    found = [...resultText(result.content).matchAll(OPENED_PR)].map((m) => Number(m[1]));
    opened.set(result, found);
  }
  return found;
}

function collectOpened(items: ChatItem[], origin: string, out: Map<number, string>): void {
  for (const it of items) {
    if (it.kind === "tool_result") {
      for (const n of openedPrs(it)) if (!out.has(n)) out.set(n, origin);
    } else if (it.kind === "tool_call" && it.children) {
      collectOpened(it.children, origin, out);
    }
  }
}

/** PR number → the top-level tool_use id of the sub-agent that opened it. A PR
 *  opened by a nested sub-agent is credited to the top-level launch it runs
 *  under (that is the thread the sidebar has a row for); the main agent's own
 *  results are not attributed. Keyed by number alone: a multi-repo agent with
 *  the same number in two repos credits the first. */
export function prOrigins(log: ChatItem[]): Map<number, string> {
  const out = new Map<number, string>();
  for (const it of log) {
    if (it.kind === "tool_call" && it.children) collectOpened(it.children, it.id, out);
  }
  return out;
}
