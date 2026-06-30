import type { ChatAdapter } from "@/adapters/types";
import { normalizeTranscript } from "./normalize";
import { codexPolicy } from "./policy";
import { reduce } from "./reduce";
import { extractUsage } from "./usage";

// Reduces Codex's `codex exec --json` thread/turn/item event stream
// (verified against codex-cli 0.135.0 — see ./reduce.ts). Transcript
// replay on re-attach is a follow-up (see ./normalize.ts).
export const codexAdapter: ChatAdapter = {
  id: "codex",
  reduce,
  normalizeTranscript,
  policy: codexPolicy,
  extractUsage,
};
