// OpenCode transcript replay isn't wired in v1.
//
// OpenCode has an `export <session-id>` command, but its on-disk schema
// differs from the live `run --format json` event stream the reducer
// consumes, so mapping it is a follow-up. The Rust transport returns an
// empty transcript for opencode agents (see `read_session_transcript`);
// re-attaching replays from the provider-agnostic SQLite event log, and
// `--session <id>` continues the conversation.

import type { RawEvent } from "../types";

export function normalizeTranscript(_lines: unknown[]): RawEvent[] {
  return [];
}
