import type { ChatAdapter } from "../types";
import { reduce } from "./reduce";
import { normalizeTranscript } from "./normalize";
import { cursorPolicy } from "./policy";

// Cursor Agent's stream-json is Claude Code's schema except for tool calls
// (see ./reduce.ts), so most of the adapter delegates to the Claude reducer.
export const cursorAdapter: ChatAdapter = {
  id: "cursor",
  reduce,
  normalizeTranscript,
  policy: cursorPolicy,
};
