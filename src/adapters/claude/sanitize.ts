// Strips Claude Code internal wrapper tags from user-message text and
// replaces them with structured notices. The tags are injected by the
// claude CLI before passing the prompt to the model — they aren't
// user-authored content and shouldn't render as user bubbles.

import type { ChatItem } from "@/adapters/types";
import { stripInjectedInstructions } from "@/util/instructions";

type NoticeItem = Extract<ChatItem, { kind: "notice" }>;

export interface SanitizeResult {
  text: string;
  notices: NoticeItem[];
}

// Tag patterns. `[\s\S]` so they cross newlines (claude wrappers span
// lines for command stdout); `?` for non-greedy matching so multiple
// tags in one message don't merge.
const COMMAND_NAME_RE = /<command-name>([\s\S]*?)<\/command-name>/g;
const COMMAND_SIBLINGS_RE =
  /<(command-message|command-args|local-command-stdout|local-command-stderr|local-command-caveat)>[\s\S]*?<\/\1>/g;
const SYSTEM_REMINDER_RE = /<system-reminder>([\s\S]*?)<\/system-reminder>/g;

// Claude emits this preamble as a synthetic user-role event right after
// a /compact finishes. The body is the summary of the prior context;
// surfacing it as a user bubble is misleading because the user didn't
// type it. Convert to a compact_summary notice instead.
const COMPACT_PREAMBLE_RE = /^This session is being continued from a previous conversation/;

// When a background Task/Bash finishes, the harness re-invokes the agent with
// a `<task-notification>` user-role event (origin.kind === "task-notification").
// It carries neither isMeta nor isSynthetic, so it isn't caught by the injected
// guard in reduce.ts and would otherwise render its raw XML as a user bubble.
// Detect it by the wrapper tag — present on both the live wire and the persisted
// transcript, unlike origin which the CLI reshapes between the two — and surface
// only the human-readable <summary> as a quiet background_task notice. Stripped
// inline (global, like the other wrappers) so a notification riding alongside
// real user text doesn't leak its raw XML into the bubble.
const TASK_NOTIFICATION_RE = /<task-notification>([\s\S]*?)<\/task-notification>/g;
const TASK_SUMMARY_RE = /<summary>([\s\S]*?)<\/summary>/;
const TASK_STATUS_RE = /<status>([\s\S]*?)<\/status>/;

function taskNotificationNotice(body: string): NoticeItem | null {
  const summary = body.match(TASK_SUMMARY_RE)?.[1]?.trim();
  const status = body.match(TASK_STATUS_RE)?.[1]?.trim() ?? "";
  // A contentless notification (no summary and no status) carries nothing worth
  // showing — drop it like an empty system-reminder rather than emitting a
  // misleading "Background task update" line.
  if (!summary && !status) return null;
  const text = summary || `Background task ${status}`;
  // Anything other than a clean completion (e.g. "stopped", a nonzero exit)
  // flags the dot red; the summary text already spells out the detail.
  const isError = status !== "" && status !== "completed" && status !== "success";
  return isError
    ? { kind: "notice", subtype: "background_task", text, is_error: true }
    : { kind: "notice", subtype: "background_task", text };
}

// Cursor (which reuses this sanitizer) wraps every user turn in its own
// envelope: a `<timestamp>` line followed by the query inside `<user_query>`.
// Neither is user-authored. Claude never emits these, so it's a no-op there.
const CURSOR_TIMESTAMP_RE = /<timestamp>[\s\S]*?<\/timestamp>/g;
const CURSOR_USER_QUERY_RE = /<user_query>([\s\S]*?)<\/user_query>/;

export function sanitizeUserText(raw: string): SanitizeResult {
  const notices: NoticeItem[] = [];

  if (COMPACT_PREAMBLE_RE.test(raw.trimStart())) {
    return {
      text: "",
      notices: [{ kind: "notice", subtype: "compact_summary", text: "Conversation compacted" }],
    };
  }

  // Background-task notifications → background_task notices. Stripped inline so
  // surrounding user text survives; contentless ones drop to nothing.
  let text = raw.replace(TASK_NOTIFICATION_RE, (_match, body: string) => {
    const notice = taskNotificationNotice(body);
    if (notice) notices.push(notice);
    return "";
  });

  // Slash-command name → one notice per invocation. Strip the tag.
  text = text.replace(COMMAND_NAME_RE, (_match, body: string) => {
    const name = body.trim();
    if (name) {
      notices.push({
        kind: "notice",
        subtype: "slash_command",
        text: name.startsWith("/") ? name : `/${name}`,
      });
    }
    return "";
  });

  // Strip the sibling tags that accompany a slash command. Their bodies
  // (e.g. local-command-stdout) are not user-facing.
  text = text.replace(COMMAND_SIBLINGS_RE, "");

  // System reminders → hook_output notices.
  text = text.replace(SYSTEM_REMINDER_RE, (_match, body: string) => {
    const inner = body.trim();
    if (inner) {
      notices.push({ kind: "notice", subtype: "hook_output", text: inner });
    }
    return "";
  });

  // Unwrap Cursor's user-turn envelope (no-op for Claude).
  text = text.replace(CURSOR_TIMESTAMP_RE, "");
  const cursorQuery = text.match(CURSOR_USER_QUERY_RE);
  if (cursorQuery) {
    text = cursorQuery[1];
  }

  // Strip the Fletch-injected instruction block at the data layer (not just at
  // render) so the stored text equals what the user typed — this lets dedup
  // merge the agent's echoed turn with the optimistic one. No-op when absent.
  text = stripInjectedInstructions(text);

  return { text: text.trim(), notices };
}
