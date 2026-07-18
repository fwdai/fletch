/** One canonical record from session_records: a verbatim per-provider
 *  transcript body plus its dedup key and provenance. */
export interface SessionRecord {
  seq: number;
  provider: string;
  source: string;
  native_id: string;
  agent_version: string | null;
  body: Record<string, unknown> & { type?: string };
}

/** One Fletch-origin outgoing user message (session_user_turns). Carries the
 *  attachment metadata the transcript lacks; `native_id` links it to the
 *  canonical session_records user-message once matched at turn-end (null =
 *  pending or failed — rendered standalone for retry). */
export interface UserTurn {
  turn_id: string;
  seq: number;
  text: string;
  attachments: string[];
  native_id: string | null;
  /** Epoch millis when the turn started running; null if it never started. */
  started_at: number | null;
  /** Epoch millis when the turn finished; null while in flight. */
  ended_at: number | null;
}

export interface SessionRecordsAppendedEvent {
  agent_id: string;
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

export interface TurnStartedEvent {
  agent_id: string;
  /** Backend epoch millis the turn began — the live-timer anchor. */
  started_at: number;
}
