import type { ChatItem } from "@/adapters";
import type { ForkContext } from "@/api";
import { renderToolResult, stringifyInput } from "@/components/Workspace/messages/presenters/util";
import { APP_ACTION_PREFIX } from "@/delegation";
import { stripInjectedInstructions } from "@/util/instructions";

// Assembles the prose a fork carries into the child agent's brief. The caller
// (workspace.ts forkAgent) feeds these the parent's record-derived,
// policy-filtered history — the same history the child shows through its
// session lineage — so the injected context never diverges from what the child
// displays, for any provider.

/** Serialize one chat item into a line of the fork brief, or null to skip it.
 *  Covers every kind the child transcript can render (tool calls/results,
 *  reasoning, error notices) — not just messages — so the injected context
 *  carries the tool output and diagnostics the inherited history shows. */
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
    // Optimistic, store-only item never present in stored records.
    case "queued_message":
      return null;
  }
}

/** Assemble the prose a fork carries into the child's brief. Mirrors the
 *  backend's cut: everything through the anchor turn, stopping at the next turn
 *  (a user message carrying a turn id); the whole log when the anchor is the
 *  end (`turn_id: null`). Returns null when nothing is carried, or when the
 *  anchor isn't in the log. */
export function forkContextDigest(log: ChatItem[], context: ForkContext): string | null {
  if (context.kind === "none") return null;

  const turnIdOf = (it: ChatItem) => (it.kind === "user_message" ? it.turnId : undefined);

  // Exclusive item cutoff.
  let cutoff = log.length;
  if (context.turn_id !== null) {
    const anchorId = context.turn_id;
    const anchor = log.findIndex((it) => turnIdOf(it) === anchorId);
    if (anchor === -1) return null;
    const next = log.findIndex((it, i) => i > anchor && turnIdOf(it) !== undefined);
    if (next !== -1) cutoff = next;
  }

  const lines: string[] = [];
  for (let i = 0; i < cutoff; i += 1) {
    const line = serializeForkItem(log[i]);
    if (line) lines.push(line);
  }
  const digest = lines.join("\n\n").trim();
  return digest.length > 0 ? digest : null;
}
