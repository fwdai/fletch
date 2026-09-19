// Claude Code background tasks — sub-agents (`Agent` tool with
// `run_in_background`) and background Bash — reduced from the live stream's
// top-level `system` events. These keep flowing after the main turn's `result`
// has landed and the agent has gone idle, so a chat can show a sub-agent still
// working (or having failed) under a quiet row. The chat reducer ignores
// `system` events; this module is their only consumer.
//
// Pure and framework-free on purpose: the desktop store (store/backgroundTasks)
// and the mobile app's own store both fold events through `applyTaskEvent` and
// read the map through the selectors below.
//
// Verified wire shapes (claude --print --output-format stream-json):
//   task_started            {task_id, tool_use_id, description, task_type,
//                            subagent_type?, is_backgrounded, spawn_depth?,
//                            prompt?, owned_by_subagent?}
//   task_progress           {task_id, description (current activity),
//                            subagent_type, usage, last_tool_name}
//                            — once per sub-agent tool use; Bash sends none
//   task_updated            {task_id, patch: {status, end_time}}
//   task_notification       {task_id, status, summary, output_file, usage?}
//                            — `usage` only for sub-agents; no task_type
//   background_tasks_changed {tasks: [{task_id, task_type, description}]}
//                            — the authoritative live list
// A completed task_id can `task_started` again (the sub-agent was resumed by a
// message); that is a fresh run.

import type { RawEvent } from "@/adapters/types";
import { asRecord, isRecord } from "./json";

export type BackgroundTaskStatus = "running" | "completed" | "failed";

export type BackgroundTask = {
  taskId: string;
  /** The Agent/Bash tool_use that launched it — the chat's tool_call id. */
  toolUseId: string;
  /** The task's name as given at launch (not the latest progress activity). */
  description: string;
  /** `unknown` when the task was first seen through an event that names no
   *  type (a progress/notification with no prior start, e.g. after a
   *  reconnect) and carried nothing that gives the type away. Upgraded in
   *  place by a later `task_started` or `background_tasks_changed`. */
  taskType: "local_agent" | "local_bash" | "unknown" | string;
  subagentType?: string;
  backgrounded: boolean;
  /** True when a sub-agent (not the main agent) launched this task. */
  ownedBySubagent: boolean;
  spawnDepth?: number;
  prompt?: string;
  status: BackgroundTaskStatus;
  startedAt: number;
  /** Last time any event touched this task — the "quiet for" anchor. */
  lastActivityAt: number;
  endedAt?: number;
  toolUses: number;
  totalTokens: number;
  durationMs: number;
  lastToolName?: string;
  summary?: string;
  outputFile?: string;
  /** The non-`completed` status a task ended with (`stopped`, `failed`, …). */
  failureStatus?: string;
};

/** One agent's tasks, keyed by task_id. */
export type BackgroundTaskMap = Record<string, BackgroundTask>;

const TASK_SUBTYPES = new Set([
  "task_started",
  "task_progress",
  "task_updated",
  "task_notification",
  "background_tasks_changed",
]);

/** True for the `system` events this module consumes. */
export function isTaskEvent(ev: RawEvent): boolean {
  return ev.type === "system" && typeof ev.subtype === "string" && TASK_SUBTYPES.has(ev.subtype);
}

const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
const num = (v: unknown): number | undefined => (typeof v === "number" ? v : undefined);

/** Cumulative usage as Claude reports it on progress/notification events. */
function withUsage(task: BackgroundTask, usage: unknown): BackgroundTask {
  const u = asRecord(usage);
  return {
    ...task,
    toolUses: num(u.tool_uses) ?? task.toolUses,
    totalTokens: num(u.total_tokens) ?? task.totalTokens,
    durationMs: num(u.duration_ms) ?? task.durationMs,
  };
}

/** The type an event gives away when it names none: only sub-agents report a
 *  `subagent_type` or cumulative `usage`. A Bash notification carries neither,
 *  and its `summary` wording is not evidence — that stays `unknown` rather
 *  than being guessed, so a missed-start Bash failure never reads as a
 *  failed sub-agent. */
function taskTypeFrom(ev: RawEvent, prev?: BackgroundTask): string {
  const named = str(ev.task_type);
  if (named) return named;
  if (prev && prev.taskType !== "unknown") return prev.taskType;
  if (str(ev.subagent_type) || isRecord(ev.usage)) return "local_agent";
  return "unknown";
}

/** A running task built from whatever identifying fields `ev` carries — used
 *  both for `task_started` and as the fallback when a later event names a task
 *  we never saw start (e.g. the client attached mid-run). */
function startedFrom(ev: RawEvent, now: number, prev?: BackgroundTask): BackgroundTask {
  return {
    taskId: String(ev.task_id),
    toolUseId: str(ev.tool_use_id) ?? prev?.toolUseId ?? "",
    description: str(ev.description) ?? prev?.description ?? "",
    taskType: taskTypeFrom(ev, prev),
    subagentType: str(ev.subagent_type) ?? prev?.subagentType,
    backgrounded: ev.is_backgrounded === true || (prev?.backgrounded ?? false),
    ownedBySubagent: ev.owned_by_subagent === true || (prev?.ownedBySubagent ?? false),
    spawnDepth: num(ev.spawn_depth) ?? prev?.spawnDepth,
    prompt: str(ev.prompt) ?? prev?.prompt,
    status: "running",
    startedAt: now,
    lastActivityAt: now,
    // Claude's usage is cumulative across a resumed task, so carry it over.
    toolUses: prev?.toolUses ?? 0,
    totalTokens: prev?.totalTokens ?? 0,
    durationMs: prev?.durationMs ?? 0,
    lastToolName: prev?.lastToolName,
  };
}

function ended(
  task: BackgroundTask,
  status: string | undefined,
  now: number,
  endedAt?: number,
): BackgroundTask {
  const completed = status === undefined || status === "completed";
  return {
    ...task,
    status: completed ? "completed" : "failed",
    failureStatus: completed ? undefined : status,
    endedAt: endedAt ?? now,
    lastActivityAt: now,
  };
}

/** Fold one live event into an agent's task map. Returns `prev` itself (same
 *  reference) for events it does not handle or that change nothing. */
export function applyTaskEvent(
  prev: BackgroundTaskMap,
  ev: RawEvent,
  now: number,
): BackgroundTaskMap {
  if (!isTaskEvent(ev)) return prev;

  if (ev.subtype === "background_tasks_changed") return reconcile(prev, ev, now);

  const taskId = str(ev.task_id);
  if (!taskId) return prev;
  const existing = prev[taskId];

  switch (ev.subtype) {
    case "task_started":
      return { ...prev, [taskId]: startedFrom(ev, now, existing) };

    case "task_progress": {
      // `description` here is the current activity, not the task's name — keep
      // the launch description and only take the usage/tool fields.
      const base = existing ?? startedFrom({ ...ev, description: undefined }, now);
      const next = withUsage({ ...base, status: "running", lastActivityAt: now }, ev.usage);
      next.lastToolName = str(ev.last_tool_name) ?? next.lastToolName;
      return { ...prev, [taskId]: next };
    }

    case "task_updated": {
      if (!existing) return prev;
      const patch = asRecord(ev.patch);
      const status = str(patch.status);
      // Only terminal statuses end a task; anything still in flight is a no-op.
      if (!status || status === "running" || status === "pending") return prev;
      return { ...prev, [taskId]: ended(existing, status, now, num(patch.end_time)) };
    }

    case "task_notification": {
      const base = existing ?? startedFrom(ev, now);
      const next = withUsage(ended(base, str(ev.status), now), ev.usage);
      next.summary = str(ev.summary) ?? next.summary;
      next.outputFile = str(ev.output_file) ?? next.outputFile;
      return { ...prev, [taskId]: next };
    }

    default:
      return prev;
  }
}

/** `background_tasks_changed` is the authoritative live list: a task we hold
 *  as running that it omits has ended (completed, absent any failure info), and
 *  a task it lists that we never saw start is running. */
function reconcile(prev: BackgroundTaskMap, ev: RawEvent, now: number): BackgroundTaskMap {
  const live = Array.isArray(ev.tasks) ? ev.tasks.map(asRecord) : [];
  const liveIds = new Set(live.map((t) => str(t.task_id)).filter((id): id is string => !!id));
  let next: BackgroundTaskMap | undefined;

  for (const task of Object.values(prev)) {
    if (task.status === "running" && !liveIds.has(task.taskId)) {
      next ??= { ...prev };
      next[task.taskId] = ended(task, undefined, now);
    }
  }
  for (const t of live) {
    const id = str(t.task_id);
    if (!id) continue;
    const cur = prev[id];
    if (cur?.status === "running") {
      // Already tracked; the only news the list can bring is the type a
      // start-less adoption could not tell.
      const type = str(t.task_type);
      if (cur.taskType !== "unknown" || !type) continue;
      next ??= { ...prev };
      next[id] = { ...cur, taskType: type };
      continue;
    }
    next ??= { ...prev };
    next[id] = startedFrom({ ...t, is_backgrounded: true }, now, cur);
  }
  return next ?? prev;
}

// ── selectors ────────────────────────────────────────────────────────────

/** Running tasks, oldest start first. */
export function liveBackgroundTasks(tasks: BackgroundTaskMap): BackgroundTask[] {
  return Object.values(tasks)
    .filter((t) => t.status === "running")
    .sort((a, b) => a.startedAt - b.startedAt);
}

/** Millis since the task last reported anything. */
export function quietForMs(task: BackgroundTask, now: number): number {
  return Math.max(0, now - task.lastActivityAt);
}

export type SubagentActivity = {
  running: number;
  failed: number;
  /** The longest silence among running tasks; undefined when none is running. */
  quietest: number | undefined;
};

/** A sub-agent, as opposed to background Bash or a task whose type we could
 *  not tell. Consumers that count or list sub-agents should start from this
 *  (and add their own `ownedBySubagent` filter for top-level only). */
export const isSubagentTask = (t: BackgroundTask): boolean => t.taskType === "local_agent";

/** A cheap summary for a sidebar row: how many sub-agents are running, how
 *  many have failed, and how long the quietest running one has been silent.
 *  Background Bash and `unknown`-typed tasks are not counted. */
export function subagentActivity(tasks: BackgroundTaskMap, now: number): SubagentActivity {
  let running = 0;
  let failed = 0;
  let quietest: number | undefined;
  for (const t of Object.values(tasks)) {
    if (!isSubagentTask(t)) continue;
    if (t.status === "failed") failed += 1;
    if (t.status !== "running") continue;
    running += 1;
    const quiet = quietForMs(t, now);
    if (quietest === undefined || quiet > quietest) quietest = quiet;
  }
  return { running, failed, quietest };
}
