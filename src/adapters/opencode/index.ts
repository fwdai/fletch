import type { ChatAdapter } from "../types";
import { reduce } from "./reduce";
import { normalizeTranscript } from "./normalize";
import { opencodePolicy } from "./policy";
import { extractUsage } from "./usage";

// Reduces OpenCode's `opencode run --format json` step/part event stream
// (verified against opencode 1.15.12 — see ./reduce.ts). Transcript replay
// on re-attach is a follow-up (see ./normalize.ts).
export const opencodeAdapter: ChatAdapter = {
  id: "opencode",
  reduce,
  normalizeTranscript,
  policy: opencodePolicy,
  extractUsage,
};
