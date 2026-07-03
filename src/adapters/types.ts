// Multi-agent chat adapter contracts. Every per-agent adapter
// produces values from the ChatItem union; the renderer is agnostic
// to which adapter produced any given item.
//
// See docs/superpowers/specs/2026-05-27-multi-agent-chat-adapters-design.md
// for the design rationale.

export type ChatItem =
  | {
      kind: "user_message";
      text: string;
      attachments?: string[];
      /** Run timing for the turn this message starts, overlaid from the
       *  matching `UserTurn` row (epoch millis). `endedAt` null = still in
       *  flight (the live turn); both absent for turns with no timing row. */
      startedAt?: number;
      endedAt?: number;
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
    };

export type NoticeSubtype =
  | "turn_end"
  | "error"
  | "info"
  | "reasoning"
  | "slash_command"
  | "compact_summary"
  | "hook_output";

export type RawEvent = Record<string, unknown> & { type?: string };

export type DisplayMode = "show" | "hide";

// Keys are either `${kind}` or `${kind}:${subtype}`. The more specific
// `${kind}:${subtype}` entry wins when both are present.
export type DisplayPolicy = Record<string, DisplayMode>;

/** Normalized token usage extracted from ONE persisted session_record body.
 *
 *  Unlike `reduce`/`normalizeTranscript`, the usage extractors read each
 *  agent's ON-DISK transcript body shape directly (see `<agent>/usage.ts`),
 *  not the live event stream. Usage is folded over session_records — the
 *  canonical store — rather than the ephemeral live stream, so cumulative
 *  totals survive restarts and never double-count a turn rendered both live
 *  and from records. See src/adapters/usage.ts for the fold. */
export interface TurnUsage {
  /** When true, the fields are running cumulative totals and the latest record
   *  wins; when false/absent they are a per-record delta and are summed. Codex
   *  reports cumulative `total_token_usage`; claude/opencode/pi report deltas. */
  cumulative?: boolean;
  /** Fresh, non-cached input tokens. */
  inputTokens: number;
  /** Output tokens, including reasoning/thinking tokens. */
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  /** Dollar cost, only for agents that report it natively (opencode, pi). */
  costUsd?: number;
  /** This record's context-window composition (latest record wins in the fold).
   *  Its parts sum to the window fill and drive the meter's segmented bar:
   *  `cacheRead` = reused/cached context, `cacheWrite` = newly cached this turn,
   *  `input` = fresh non-cached input. The semantic split the design mocks up
   *  (system / conversation / reasoning) is NOT recoverable from any agent's
   *  transcript — this cache-state split is the truthful equivalent. */
  context?: { input: number; cacheRead: number; cacheWrite: number };
  /** Model context window size in tokens, when the agent reports it (codex). */
  contextWindow?: number;
  model?: string;
}

export interface ChatAdapter {
  readonly id: string;
  reduce(prevItems: ChatItem[], rawEvent: RawEvent): ChatItem[];
  normalizeTranscript(transcriptLines: unknown[]): RawEvent[];
  readonly policy: DisplayPolicy;
  /** True when the agent emits usage ONLY on its live `result` event and never
   *  persists it on disk (cursor). The store persists that event into
   *  session_records (`source = 'live_compiled'`) at turn-end so usage is then
   *  folded uniformly from records like every other agent — restart-safe, no
   *  in-memory accumulation. `extractUsage` reads that same `result` body. */
  readonly persistLiveUsage?: boolean;
  /** Extract token usage from one session_record body, or undefined when it
   *  carries none. Optional: agents that report no usage at all (antigravity)
   *  omit it entirely. */
  extractUsage?(recordBody: RawEvent): TurnUsage | undefined;
}
