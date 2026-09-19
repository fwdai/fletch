// Cursor's `Task` tool — a sub-agent — as it arrives on the live stream: a
// `tool_call` event whose payload key is `taskToolCall`. Shared by the chat
// reducer (the tool row and its result) and `taskEvents` (the background-task
// store), so the payload is read in one place.
//
// Field names come from cursor-agent's bundled protos (2026.06.19), serialized
// the way every captured Cursor tool payload is (protobuf-es defaults: camelCase
// JSON names — `exitCode`, `globPattern`, `interleavedOutput`, …):
//   agent.v1.TaskToolCall  {args: TaskArgs, result: TaskResult}
//   TaskArgs               {description, prompt, subagentType, model, …}
//     subagentType is a oneof message — {explore: {}} / {custom: {name}} / … —
//     where the on-disk transcript flattens it to a string ("explore",
//     "ci-investigator"). Both forms are read.
//   TaskResult             {success: TaskSuccess} | {error: {error}}
//   TaskSuccess            {conversationSteps: [{assistantMessage: {text}} |
//                           {toolCall} | {thinkingMessage}], durationMs,
//                           resultSuffix, isBackground, …}
// No live `taskToolCall` event has been captured yet, so every read below has
// a fallback: an unexpected shape renders as the raw payload, never as nothing.

import { asRecord } from "@/adapters/shared/json";
import type { RawEvent } from "@/adapters/types";

export type CursorTaskCall = {
  callId: string;
  /** `started` / `completed` (other in-flight subtypes pass through as-is). */
  subtype: string;
  /** The Task's args with `subagent_type` as a string — Claude's field name,
   *  so the shared Agent presenter renders type/description/prompt. */
  input: Record<string, unknown>;
  result?: Record<string, unknown>;
};

/** The sub-agent type as one string: the flattened form as-is, or the oneof's
 *  variant name (a custom type by its `name`). */
export function subagentTypeName(v: unknown): string | undefined {
  if (typeof v === "string") return v || undefined;
  const rec = asRecord(v);
  const key = Object.keys(rec)[0];
  if (!key) return undefined;
  const name = asRecord(rec[key]).name;
  return typeof name === "string" && name ? name : key;
}

/** The Task call in `ev`, or null for any other event. */
export function cursorTaskCall(ev: RawEvent): CursorTaskCall | null {
  if (ev.type !== "tool_call") return null;
  const tc = asRecord(ev.tool_call);
  if (!("taskToolCall" in tc)) return null;
  const callId = String(ev.call_id ?? "");
  if (!callId) return null;
  const inner = asRecord(tc.taskToolCall);
  const args = asRecord(inner.args);
  const input: Record<string, unknown> = { ...args };
  const subagentType = subagentTypeName(args.subagentType ?? args.subagent_type);
  if (subagentType) input.subagent_type = subagentType;
  return {
    callId,
    subtype: typeof ev.subtype === "string" ? ev.subtype : "",
    input,
    result: inner.result === undefined ? undefined : asRecord(inner.result),
  };
}

export type CursorTaskOutcome = {
  content: unknown;
  isError: boolean;
  durationMs?: number;
};

/** What a completed Task reports: the error text; else the sub-agent's final
 *  message when the result carries its steps; else the raw result. */
export function cursorTaskOutcome(call: CursorTaskCall): CursorTaskOutcome {
  const result = call.result ?? {};
  if ("error" in result) {
    const message = asRecord(result.error).error;
    return { content: typeof message === "string" ? message : result.error, isError: true };
  }
  const success = asRecord(result.success);
  const steps = Array.isArray(success.conversationSteps)
    ? success.conversationSteps.map(asRecord)
    : [];
  const finalText = steps
    .map((step) => asRecord(step.assistantMessage).text)
    .filter((text): text is string => typeof text === "string" && text.length > 0)
    .pop();
  // 64-bit protobuf ints serialize as strings in JSON; accept either.
  const duration = Number(success.durationMs);
  return {
    content:
      finalText ??
      (typeof success.resultSuffix === "string" ? success.resultSuffix : (call.result ?? "")),
    isError: false,
    durationMs:
      success.durationMs !== undefined && Number.isFinite(duration) ? duration : undefined,
  };
}

/** Claude-shaped `system` task events for one Cursor event, so the store folds
 *  a Cursor sub-agent through the same `applyTaskEvent` reducer as Claude's
 *  (see shared/backgroundTasks). Empty for anything but a Task `tool_call`.
 *
 *  `started` → `task_started`; `completed` → `task_notification`. The task is
 *  keyed by the call id (Cursor has no separate task id) and is never
 *  backgrounded: whether `-p` mode lets a sub-agent outlive the turn is
 *  unverified, and a `completed` payload arrives for it either way. */
export function taskEvents(ev: RawEvent): RawEvent[] {
  const call = cursorTaskCall(ev);
  if (!call) return [];
  const base = {
    type: "system",
    task_id: call.callId,
    tool_use_id: call.callId,
    task_type: "local_agent",
  };
  if (call.subtype === "started") {
    return [
      {
        ...base,
        subtype: "task_started",
        description: call.input.description,
        subagent_type: call.input.subagent_type,
        prompt: call.input.prompt,
        is_backgrounded: false,
      },
    ];
  }
  if (call.subtype === "completed") {
    const outcome = cursorTaskOutcome(call);
    const notification: RawEvent = {
      ...base,
      subtype: "task_notification",
      status: outcome.isError ? "failed" : "completed",
    };
    if (typeof outcome.content === "string") notification.summary = outcome.content;
    if (outcome.durationMs !== undefined) notification.usage = { duration_ms: outcome.durationMs };
    return [notification];
  }
  return [];
}
