// Multi-agent chat adapter contracts. Every per-agent adapter
// produces values from the ChatItem union; the renderer is agnostic
// to which adapter produced any given item.
//
// See docs/superpowers/specs/2026-05-27-multi-agent-chat-adapters-design.md
// for the design rationale.

import type { Coverage, UsageEvent } from "./usage/events";

export type ChatItem = ChatItemBase & ChatItemBody;

interface ChatItemBase {
  /** The `seq` of the `session_records` row this item was reduced from, so an
   *  item can be placed by its position in the record stream rather than by
   *  matching its text. Absent for items folded from the live stream or created
   *  store-only. An item a later record extends (a streaming message, an
   *  upserted tool call) keeps the seq of the record that created it. */
  recordSeq?: number;
  /** The session that produced the `recordSeq` record. A forked history
   *  stitches several sessions whose seqs overlap, so the pair is what names
   *  the record. Absent wherever `recordSeq` is, and from hosts that predate
   *  it. */
  recordSession?: string;
}

type ChatItemBody =
  | {
      kind: "user_message";
      text: string;
      attachments?: string[];
      /** Run timing for the turn this message starts, overlaid from the
       *  matching `UserTurn` row (epoch millis). `endedAt` null = still in
       *  flight (the live turn); both absent for turns with no timing row. */
      startedAt?: number;
      endedAt?: number;
      /** The turn's id (`session_user_turns.turn_id`): on a store-inserted
       *  optimistic bubble (the message that opened a turn, before the
       *  transcript echoes it), or overlaid from the matching `UserTurn` row.
       *  Lets the `turn:sent` mirror skip a send already in the log, and names
       *  the turn a fork anchors on. Never set by an adapter's reduce(). */
      turnId?: string;
      /** The agent never read this message: its turn was stopped before the
       *  provider logged the prompt (`interrupted`) or it was never delivered
       *  (`failed`). Set from the turn row's outcome (see `mergeUserTurns`);
       *  the bubble says so and offers to send it again. */
      undelivered?: "interrupted" | "failed";
    }
  // A follow-up the user sent mid-turn that hasn't landed in the transcript
  // yet: delivered live into the running turn (claude) or queued for the next
  // turn boundary (per-turn agents). Store-inserted only — never produced by an
  // adapter's reduce(). Reconciled away once the canonical transcript catches
  // up (see app.ts onSessionRecordsAppended). Renders with the user bubble.
  | {
      kind: "queued_message";
      text: string;
      attachments?: string[];
      /** True only while the message is genuinely held for a later turn
       *  boundary (per-turn agents, or claude paused on a tool gate). A message
       *  injected/delivered now clears this and renders as a plain bubble. Set
       *  from the send's delivery outcome. */
      queued?: boolean;
      /** Client turn id, used to locate this optimistic item and flip `queued`
       *  once the backend reports the delivery outcome. */
      turnId?: string;
    }
  | {
      kind: "agent_message";
      text: string;
      streaming?: boolean;
      /** The model that produced this turn, when the agent reports it in its
       *  transcript: Claude/pi `message.model` (live + replay); Codex
       *  `turn_context.model` and OpenCode message-blob `modelID` (replay only —
       *  their live streams omit it). Absent for Cursor (model only in the live
       *  `init` event, not the on-disk transcript) and Antigravity (no model in
       *  the transcript) — consumers fall back to the static provider label. */
      model?: string;
    }
  | {
      kind: "tool_call";
      id: string;
      name: string;
      input: unknown;
      streaming?: boolean;
      /** Scratch space for assembling `input` from stream deltas, present only
       *  while `streaming`. `block` is the content-block index this call
       *  occupies in the in-flight assistant message — Claude keys its
       *  `input_json_delta` fragments by it — and `json` is the raw JSON text
       *  accumulated so far, which `input` is the best-effort parse of.
       *  Cleared when the finalized event lands. */
      partial?: { block: number; json: string };
      /** Sub-conversation produced by a subagent spawned through this tool
       *  call (Claude's Task/Agent tool). The reducer routes sidechain events
       *  — those the SDK tags with `parent_tool_use_id === this id` — into a
       *  nested ChatItem log here instead of the main timeline, so the
       *  subagent's reasoning/tool use threads under its row rather than
       *  leaking into the chat. Absent for ordinary tool calls. */
      children?: ChatItem[];
    }
  | {
      kind: "tool_result";
      tool_use_id: string;
      content: unknown;
      is_error?: boolean;
    }
  | {
      kind: "notice";
      subtype: NoticeSubtype;
      text: string;
      is_error?: boolean;
      /** For `command_output`: the invoked command, shown as the block header
       *  (e.g. "/doctor"). Ignored by other subtypes. */
      label?: string;
      /** For `compact_summary`: the summary of the earlier conversation the
       *  agent continued from. Not rendered; a handoff transcript starts from
       *  it (see adapters/handoff). */
      summary?: string;
    };

export type NoticeSubtype =
  | "turn_end"
  | "error"
  | "info"
  | "reasoning"
  | "slash_command"
  | "compact_summary"
  | "hook_output"
  | "background_task"
  /** Output of a user-invoked local slash command (e.g. `/doctor`). Rendered
   *  as a prominent, readable block — not the dim ambient-notice style — since
   *  the user asked for it and expects to read it. */
  | "command_output";

export type RawEvent = Record<string, unknown> & {
  type?: string;
  /** The `seq` of the session record this event was normalized from (see
   *  `shared/record-seq`). Set only by `normalizeTranscript` when given seqs;
   *  no provider payload uses this key, so stamping it can't shadow a field. */
  recordSeq?: number;
  /** The session of that record, when `normalizeTranscript` was given them. */
  recordSession?: string;
};

export type DisplayMode = "show" | "hide";

// Keys are either `${kind}` or `${kind}:${subtype}`. The more specific
// `${kind}:${subtype}` entry wins when both are present.
export type DisplayPolicy = Record<string, DisplayMode>;

export interface ChatAdapter {
  readonly id: string;
  reduce(prevItems: ChatItem[], rawEvent: RawEvent): ChatItem[];
  /** Translate transcript lines into the events `reduce` consumes. `seqs`, when
   *  given, is parallel to `transcriptLines` (each line's session-record seq):
   *  every event is stamped with the seq of the line it was emitted for, and
   *  `reduce` carries it onto the items it creates. Omitted for bare bodies.
   *  `sessions`, parallel the same way, names each line's session and rides
   *  along with its seq. */
  normalizeTranscript(
    transcriptLines: unknown[],
    seqs?: readonly number[],
    sessions?: readonly string[],
  ): RawEvent[];
  /** Claude-shaped `system` task events (`task_started` / `task_notification`,
   *  see shared/backgroundTasks) derived from ONE live event of a provider that
   *  has no such events of its own — Cursor's Task `tool_call`. The store folds
   *  them through the same reducer as Claude's, so no background-task surface
   *  needs a provider branch. Absent for providers that emit the events
   *  natively (claude) or have no sub-agents. The event itself still goes to
   *  `reduce` afterwards. */
  taskEvents?(rawEvent: RawEvent): RawEvent[];
  readonly policy: DisplayPolicy;
  /** True when the agent emits usage ONLY on its live stream and never persists
   *  it on disk (cursor, and opencode in `run` mode). The store writes that
   *  event into session_records (`source = 'live_compiled'`) at turn-end so
   *  usage aggregates uniformly from records like every other agent — no
   *  in-memory accumulation. `usageEvents` reads that same body. */
  readonly persistLiveUsage?: boolean;
  /** Whether records can be trusted to hold every request the session made.
   *  Defaults to `complete`; `persistLiveUsage` agents are `partial`, since a
   *  turn that ran while Fletch wasn't listening left no trace to re-read. */
  readonly usageCoverage?: Coverage;
  /** Translate ONE persisted session_record body into the token-usage events it
   *  describes — see `usage/events.ts`. Unlike `reduce`/`normalizeTranscript`,
   *  this reads the agent's ON-DISK body shape (see `<agent>/usage.ts`).
   *
   *  Return what the record says HAPPENED; never pre-aggregate. How events
   *  compose — which collapse, which difference, which reset the window — is
   *  the aggregator's job, so that logic exists once instead of once per agent.
   *  Optional: agents that report no usage at all (antigravity) omit it. */
  usageEvents?(recordBody: RawEvent): UsageEvent[];
}
