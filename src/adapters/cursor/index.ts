import type { ChatAdapter } from "../types";
import { reduce } from "./reduce";
import { normalizeTranscript } from "./normalize";
import { cursorPolicy } from "./policy";
import { extractUsage } from "./usage";

// Cursor Agent's stream-json is Claude Code's schema except for tool calls
// (see ./reduce.ts), so most of the adapter delegates to the Claude reducer.
// Usage isn't on disk — it's on the live `result` event, which the store
// persists into session_records so it folds like the rest (see ./usage.ts).
export const cursorAdapter: ChatAdapter = {
  id: "cursor",
  reduce,
  normalizeTranscript,
  policy: cursorPolicy,
  persistLiveUsage: true,
  extractUsage,
};
