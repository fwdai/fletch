import type { ChatItem } from "@/adapters/types";
import { APP_ACTION_PREFIX } from "@/delegation";
import { stripInjectedInstructions } from "@/util/instructions";
import { renderToolResult, stringifyInput } from "@/util/toolText";

// The handoff transcript: a conversation rendered as plain text, for the
// summarizer that briefs a new session's agent on it (the engine's `handoff`
// module). The caller (the store's forkAgent) feeds it the parent's
// record-derived, policy-filtered history — the same history the new session
// shows through its lineage — so what the agent is told never diverges from
// what the chat displays, for any provider.

/** Max chars of a single tool input or tool result in the transcript. Tool
 *  output (file dumps, logs) dominates long conversations and says little
 *  about where the work stands. */
export const HANDOFF_TOOL_TEXT_MAX = 2_000;

/** Budget, in chars, for a whole transcript: what a summarizer reads in one
 *  go. Past it, only the most recent part is kept. */
export const HANDOFF_INPUT_MAX = 400_000;

/** First line of a transcript cut to fit `HANDOFF_INPUT_MAX`. */
export const OMITTED_NOTE =
  "[Earlier conversation omitted to fit the size limit; the most recent part follows.]";

const SEP = "\n\n";

/** Cut `text` to `HANDOFF_TOOL_TEXT_MAX` chars, noting how much was dropped. */
function capText(text: string): string {
  const max = HANDOFF_TOOL_TEXT_MAX;
  if (text.length <= max) return text;
  // Don't split a surrogate pair: a lone surrogate fails to deserialize
  // backend-side and would sink the whole fork.
  const end = /[\uD800-\uDBFF]/.test(text[max - 1]) ? max - 1 : max;
  return `${text.slice(0, end)}\n[… ${(text.length - end).toLocaleString("en-US")} more chars]`;
}

/** Flatten a tool_result payload. Unlike the chat's `renderToolResult`,
 *  non-text content blocks (images, documents) become a short placeholder
 *  instead of their JSON — a base64 screenshot is megabytes of noise. */
function toolResultText(content: unknown): string {
  if (!Array.isArray(content)) return renderToolResult(content);
  return content
    .map((block: unknown) => {
      if (typeof block === "string") return block;
      if (block && typeof block === "object") {
        if ("text" in block) return String(block.text ?? "");
        if ("type" in block) return `[${String(block.type)}]`;
      }
      return "[non-text content]";
    })
    .join("\n");
}

/** Serialize one chat item into a paragraph of the transcript, or null to
 *  skip it. Covers every kind the chat can render (tool calls/results,
 *  reasoning, error notices) — not just messages — so the summary can account
 *  for the tool output and diagnostics the history shows. Tool inputs and
 *  results are capped at `HANDOFF_TOOL_TEXT_MAX` each. */
export function serializeHandoffItem(it: ChatItem): string | null {
  switch (it.kind) {
    case "user_message":
      // App-action turns (git delegation) are machinery, not conversation.
      return it.text.startsWith(APP_ACTION_PREFIX)
        ? null
        : `User: ${stripInjectedInstructions(it.text)}`;
    case "agent_message":
      return it.text ? `Assistant: ${it.text}` : null;
    case "tool_call": {
      const input = capText(stringifyInput(it.input, 2).trim());
      const head = `Assistant used tool \`${it.name}\`${input ? `:\n${input}` : ""}`;
      // Flatten a subagent's nested conversation under the call that spawned it
      // (recursion applies the same caps to its items).
      const nested = (it.children ?? [])
        .map(serializeHandoffItem)
        .filter((line): line is string => line !== null);
      return nested.length > 0 ? `${head}\n${nested.join(SEP)}` : head;
    }
    case "tool_result": {
      const text = capText(toolResultText(it.content).trim());
      if (!text) return null;
      return `${it.is_error ? "Tool error" : "Tool result"}:\n${text}`;
    }
    case "notice":
      // The summary the agent continued from, not the "compacted" marker.
      if (it.subtype === "compact_summary") return it.summary || null;
      if (!it.text) return null;
      if (it.subtype === "reasoning") return `Assistant (thinking): ${it.text}`;
      if (it.subtype === "error") return `Error: ${it.text}`;
      return it.text;
    // Optimistic, store-only item never present in stored records.
    case "queued_message":
      return null;
  }
}

/** Render the conversation in `log` through the turn `throughTurnId` and its
 *  reply, stopping at the next turn (a user message carrying a turn id) as the
 *  backend's cut does — or the whole log when it's null. Returns null when
 *  nothing is carried, or when the anchor isn't in the log.
 *
 *  The transcript starts at the last compaction before the cut, whose summary
 *  stands for everything before it, and keeps to `HANDOFF_INPUT_MAX`. */
export function handoffTranscript(log: ChatItem[], throughTurnId: string | null): string | null {
  const turnIdOf = (it: ChatItem) => (it.kind === "user_message" ? it.turnId : undefined);

  // Exclusive item cutoff.
  let cutoff = log.length;
  if (throughTurnId !== null) {
    const anchor = log.findIndex((it) => turnIdOf(it) === throughTurnId);
    if (anchor === -1) return null;
    const next = log.findIndex((it, i) => i > anchor && turnIdOf(it) !== undefined);
    if (next !== -1) cutoff = next;
  }

  // From the last compaction before the cut, or from the top when there's none.
  let start = cutoff - 1;
  while (start >= 0 && !isCompaction(log[start])) start -= 1;
  start = Math.max(start, 0);

  const lines = log
    .slice(start, cutoff)
    .map(serializeHandoffItem)
    .filter((line): line is string => line !== null);
  const transcript = keepRecent(lines);
  return transcript.length > 0 ? transcript : null;
}

function isCompaction(it: ChatItem): boolean {
  return it.kind === "notice" && it.subtype === "compact_summary" && Boolean(it.summary);
}

/** `lines` joined — or, past `HANDOFF_INPUT_MAX`, as many of the most recent
 *  as fit whole, behind `OMITTED_NOTE`. The end of the conversation is what
 *  the new session continues from. */
function keepRecent(lines: string[]): string {
  const whole = lines.join(SEP).trim();
  if (whole.length <= HANDOFF_INPUT_MAX) return whole;
  let room = HANDOFF_INPUT_MAX - OMITTED_NOTE.length;
  const kept: string[] = [];
  for (let i = lines.length - 1; i >= 0 && lines[i].length + SEP.length <= room; i -= 1) {
    kept.push(lines[i]);
    room -= lines[i].length + SEP.length;
  }
  // One item larger than the whole budget: its end.
  if (kept.length === 0) kept.push(tailOf(lines[lines.length - 1], room - SEP.length));
  return [OMITTED_NOTE, ...kept.reverse()].join(SEP);
}

/** The last `max` chars of `text`, never starting on half a surrogate pair. */
function tailOf(text: string, max: number): string {
  let start = text.length - max;
  if (/[\uDC00-\uDFFF]/.test(text[start])) start += 1;
  return text.slice(start);
}
