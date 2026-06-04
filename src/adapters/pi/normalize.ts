// Pi transcript replay isn't wired in v1.
//
// Pi persists a `session.jsonl` of its live events under its session-dir
// (default `~/.pi/agent/sessions/<path>/`), so replay is wireable later by
// reading that file. For v1 the Rust transport returns an empty transcript
// for pi agents (see `read_session_transcript`); re-attaching replays from
// the provider-agnostic SQLite event log, and `--session <id>` continues the
// conversation.

import type { RawEvent } from "../types";

export function normalizeTranscript(_lines: unknown[]): RawEvent[] {
  return [];
}
