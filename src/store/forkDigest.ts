import type { ChatItem } from "@/adapters";
import type { ForkContext } from "@/api";
import { renderToolResult, stringifyInput } from "@/components/Workspace/messages/presenters/util";
import { APP_ACTION_PREFIX } from "@/delegation";
import { stripInjectedInstructions } from "@/util/instructions";

// Assembles the prose a fork carries into the child agent's brief. The caller
// (workspace.ts forkAgent) feeds these the SAME record-derived, policy-filtered
// chat items the child transcript renders, so the injected context never
// diverges from the copied history — for any provider.

/** Max chars of a single tool input or tool result carried into the brief.
 *  Tool output (file dumps, logs) dominates long conversations, and the whole
 *  digest reaches the child agent as one command-line argument, which the OS
 *  caps. The backend enforces a hard byte cap on the total as well. */
export const FORK_TOOL_TEXT_MAX = 2_000;

/** Cut `text` to `FORK_TOOL_TEXT_MAX` chars, noting how much was dropped. */
function capText(text: string): string {
  const max = FORK_TOOL_TEXT_MAX;
  if (text.length <= max) return text;
  // Don't split a surrogate pair: a lone surrogate fails to deserialize
  // backend-side and would sink the whole fork.
  const end = /[\uD800-\uDBFF]/.test(text[max - 1]) ? max - 1 : max;
  return `${text.slice(0, end)}\n[… ${(text.length - end).toLocaleString("en-US")} more chars]`;
}

/** Flatten a tool_result payload for the brief. Unlike the chat's
 *  `renderToolResult`, non-text content blocks (images, documents) become a
 *  short placeholder instead of their JSON — a base64 screenshot is megabytes
 *  of noise to the child. */
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

/** Serialize one chat item into a line of the fork brief, or null to skip it.
 *  Covers every kind the child transcript can render (tool calls/results,
 *  reasoning, error notices) — not just messages — so the injected context
 *  carries the tool output and diagnostics the copied history shows. Tool
 *  inputs and results are capped at `FORK_TOOL_TEXT_MAX` each. */
export function serializeForkItem(it: ChatItem): string | null {
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
        .map(serializeForkItem)
        .filter((line): line is string => line !== null);
      return nested.length > 0 ? `${head}\n${nested.join("\n\n")}` : head;
    }
    case "tool_result": {
      const text = capText(toolResultText(it.content).trim());
      if (!text) return null;
      return `${it.is_error ? "Tool error" : "Tool result"}:\n${text}`;
    }
    case "notice":
      if (!it.text) return null;
      if (it.subtype === "reasoning") return `Assistant (thinking): ${it.text}`;
      if (it.subtype === "error") return `Error: ${it.text}`;
      return it.text;
    // Optimistic, store-only item never present in copied records.
    case "queued_message":
      return null;
  }
}

/** Assemble the prose a fork carries into the child's brief. Built from the same
 *  record-derived, policy-filtered surface the child renders (see forkAgent), so
 *  it stays in step with the copied history for every provider. Mirrors the
 *  backend's record cutoff: navigable prompts only (git-action turns excluded),
 *  up to the chosen point. Returns null when nothing is carried. */
export function forkContextDigest(log: ChatItem[], context: ForkContext): string | null {
  if (context.kind === "none") return null;

  const isPrompt = (it: ChatItem) =>
    it.kind === "user_message" && !it.text.startsWith(APP_ACTION_PREFIX);

  // Exclusive item cutoff. `full` carries everything; `up_to_message` stops just
  // before the prompt that follows the selected navigable ordinal.
  let cutoff = log.length;
  if (context.kind === "up_to_message") {
    let seen = -1;
    for (let i = 0; i < log.length; i += 1) {
      if (isPrompt(log[i])) {
        seen += 1;
        if (seen === context.prompt + 1) {
          cutoff = i;
          break;
        }
      }
    }
  }

  const lines: string[] = [];
  for (let i = 0; i < cutoff; i += 1) {
    const line = serializeForkItem(log[i]);
    if (line) lines.push(line);
  }
  const digest = lines.join("\n\n").trim();
  return digest.length > 0 ? digest : null;
}
