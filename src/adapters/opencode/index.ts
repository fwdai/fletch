import type { ChatAdapter } from "@/adapters/types";
import { normalizeTranscript } from "./normalize";
import { opencodePolicy } from "./policy";
import { reduce } from "./reduce";
import { extractUsage } from "./usage";

// Reduces OpenCode's `opencode run --format json` step/part event stream
// (verified against opencode 1.15.12 — see ./reduce.ts). Transcript replay
// on re-attach is a follow-up (see ./normalize.ts).
export const opencodeAdapter: ChatAdapter = {
  id: "opencode",
  reduce,
  normalizeTranscript,
  policy: opencodePolicy,
  // `opencode run` never writes the on-disk blob store Quorum reads, so usage
  // would never fold from a transcript. Capture it from the live `step_finish`
  // stream instead — persisted into session_records like Cursor's.
  persistLiveUsage: true,
  extractUsage,
};
