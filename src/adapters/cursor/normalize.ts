// Cursor transcript replay isn't wired in v1.
//
// Cursor stores chats in an internal, undocumented format under
// `~/.cursor/chats` (no `export` command), so the Rust transport returns
// an empty transcript for cursor agents (see `read_session_transcript`).
// Live turns render via the reducer; `--resume` still continues the
// conversation. Mapping the on-disk store is a follow-up.

import type { RawEvent } from "../types";

export function normalizeTranscript(_lines: unknown[]): RawEvent[] {
  return [];
}
