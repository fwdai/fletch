import type { ChatAdapter } from "@/adapters/types";
import { normalizeTranscript } from "./normalize";
import { cursorPolicy } from "./policy";
import { reduce } from "./reduce";
import { usageEvents } from "./usage";

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
  // Live-only usage means a turn that ran while Fletch wasn't listening is gone
  // for good — the totals are a floor, and the UI is told as much.
  usageCoverage: "partial",
  usageEvents,
};
