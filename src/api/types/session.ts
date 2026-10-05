/** One canonical record from session_records: a verbatim per-provider
 *  transcript body plus its dedup key and provenance. `read_session_records`
 *  returns an agent's display history, which starts with whatever its session
 *  inherits through lineage (a fork's parent conversation). */
export interface SessionRecord {
  /** Order within the record's own session; a history that spans sessions is
   *  ordered by its position in the list, not by this. */
  seq: number;
  provider: string;
  source: string;
  native_id: string;
  agent_version: string | null;
  body: Record<string, unknown> & { type?: string };
  /** Shown from an ancestor session rather than produced by this agent —
   *  display only, never this agent's usage. Absent from hosts that predate
   *  lineage, where every record is the agent's own. */
  inherited?: boolean;
}

/** One page of an agent's display history, as `read_session_page` answers:
 *  `records` in display order, and `older` the cursor naming the page before
 *  it — null once nothing older is left. Opaque: pass it back as `before`. */
export interface SessionPage {
  records: SessionRecord[];
  older: string | null;
}

/** A session the agent's workspace superseded (a rewind's abandoned branch),
 *  with its own records — empty when the caller named it as already known. */
export interface SupersededSession {
  session_id: string;
  records: SessionRecord[];
}

/** One Fletch-origin outgoing user message (session_user_turns). Carries the
 *  attachment metadata the transcript lacks; `native_id` links it to the
 *  canonical session_records user-message once matched at turn-end (null =
 *  pending or failed — rendered standalone for retry). */
export interface UserTurn {
  /** Stable id, and what a fork anchors on. */
  turn_id: string;
  seq: number;
  text: string;
  attachments: string[];
  native_id: string | null;
  /** Epoch millis when the turn started running; null if it never started. */
  started_at: number | null;
  /** Epoch millis when the turn finished; null while in flight. */
  ended_at: number | null;
  /** From an ancestor session (see `SessionRecord.inherited`). */
  inherited?: boolean;
}

export interface SessionRecordsAppendedEvent {
  agent_id: string;
}

/** The running turn's `agent:event` payloads, as `read_live_turn` answers: what
 *  a client that missed the stream folds onto the rebuilt log. `dropped` counts
 *  events cut from the head of the turn when it outgrew the host's buffer. */
export interface LiveTurn {
  events: (Record<string, unknown> & { type?: string })[];
  dropped: number;
  /** The `seq` the agent's next `agent:event` will carry; every event above
   *  has a lower one. A live frame at or past it is not in this snapshot. */
  next_seq: number;
}

/** Degraded transcript-ingest status: the vendor CLI's home dir is gone
 *  (`no_root`), its files no longer parse (`format_drift`), or matched files
 *  couldn't be read at all (`read_error`) or only partially (`partial_read`,
 *  records ingested but the tail may be missing). `healthy` is only ever sent
 *  to clear a prior degraded status. */
export type SyncHealthStatus =
  | "healthy"
  | "no_root"
  | "format_drift"
  | "read_error"
  | "partial_read";

export interface SessionSyncHealthEvent {
  agent_id: string;
  provider: string;
  status: SyncHealthStatus;
  /** Current CLI version (for display/logging only), or null if unprobed. */
  version: string | null;
}

/** A user message the host accepted for an agent, from whichever client sent
 *  it. Every client mirrors it into the chat log (skipping its own, by
 *  `turn_id`), so a prompt typed on the phone shows on the desktop and vice
 *  versa while the turn is still running. */
export interface TurnSentEvent {
  agent_id: string;
  /** The sender's client-generated turn id — matches the optimistic bubble. */
  turn_id: string;
  text: string;
  attachments: string[];
  /** The agent was mid-turn when this arrived: a follow-up (injected live or
   *  queued), not the message that opened the turn. */
  follow_up: boolean;
}

export interface TurnStartedEvent {
  agent_id: string;
  /** Backend epoch millis the turn began — the live-timer anchor. */
  started_at: number;
}
