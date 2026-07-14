import type { ChatItem } from "@/adapters";
import type { ForkContext } from "@/api";
import { APP_ACTION_PREFIX } from "@/components/RightPanel/delegation";
import { renderToolResult, stringifyInput } from "@/components/Workspace/messages/presenters/util";
import { stripInjectedInstructions } from "@/util/instructions";

// Assembles the prose a fork carries into the child agent's brief. The caller
// (workspace.ts forkAgent) feeds these the SAME record-derived, policy-filtered
// chat items the child transcript renders, so the injected context never
// diverges from the copied history — for any provider.

/** Serialize one chat item into a line of the fork brief, or null to skip it.
 *  Covers every kind the child transcript can render (tool calls/results,
 *  reasoning, error notices) — not just messages — so the injected context
 *  carries the tool output and diagnostics the copied history shows. */
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
      const input = stringifyInput(it.input, 2).trim();
      const head = `Assistant used tool \`${it.name}\`${input ? `:\n${input}` : ""}`;
      // Flatten a subagent's nested conversation under the call that spawned it.
      const nested = (it.children ?? [])
        .map(serializeForkItem)
        .filter((line): line is string => line !== null);
      return nested.length > 0 ? `${head}\n${nested.join("\n\n")}` : head;
    }
    case "tool_result": {
      const text = renderToolResult(it.content).trim();
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
