// Which playbook the agent is running, read off the chat log rather than
// tracked: a git action is delivered as a user turn whose text starts with
// `[app-action] <name>` (`appActionMessage`), so while the agent is busy, the
// message that opened its turn says whether this is one of ours.

import { APP_ACTION_PREFIX, DELEGATION_KINDS, type DelegationKind } from "@desktop/delegation";
import type { ChatItem } from "../../../adapters";

/** Playbook trigger name → delegation kind. Every kind's trigger is its own
 *  name except `resolve`, whose playbook is `resolve-conflicts`. */
const KIND_BY_ACTION: Record<string, DelegationKind> = Object.fromEntries(
  DELEGATION_KINDS.map((k) => [k === "resolve" ? "resolve-conflicts" : k, k]),
);

/** The trigger name at the head of a user message, or null for a plain one. */
export function appActionName(text: string): string | null {
  if (!text.startsWith(APP_ACTION_PREFIX)) return null;
  return text.slice(APP_ACTION_PREFIX.length).split(/\s/, 1)[0] || null;
}

/** The playbook the agent's running turn was opened with, or null: not busy,
 *  a turn opened by a plain message, or a trigger this build does not know.
 *  The last user-side item in the log is the one that opened the turn — the
 *  optimistic `queued_message` counts, since our own send sits there until
 *  the transcript echoes it. */
export function activeDelegation(
  log: ChatItem[] | undefined,
  busy: boolean,
): DelegationKind | null {
  if (!busy || !log) return null;
  for (let i = log.length - 1; i >= 0; i -= 1) {
    const item = log[i];
    if (item.kind !== "user_message" && item.kind !== "queued_message") continue;
    const name = appActionName(item.text);
    return name ? (KIND_BY_ACTION[name] ?? null) : null;
  }
  return null;
}
