import type { ChatAdapter } from "../types";
import { reduce } from "./reduce";
import { normalizeTranscript } from "./normalize";
import { piPolicy } from "./policy";

// Reduces Pi's `pi -p --mode json` event stream (verified against pi 0.74.2 —
// see ./reduce.ts). Transcript replay on re-attach is a follow-up (see
// ./normalize.ts).
export const piAdapter: ChatAdapter = {
  id: "pi",
  reduce,
  normalizeTranscript,
  policy: piPolicy,
};
