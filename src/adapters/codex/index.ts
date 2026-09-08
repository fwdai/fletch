import type { ChatAdapter } from "@/adapters/types";
import { normalizeTranscript } from "./normalize";
import { codexPolicy } from "./policy";
import { reduce } from "./reduce";
import { usageEvents } from "./usage";

// Reduces Codex's `codex exec --json` thread/turn/item event stream
// (verified against codex-cli 0.135.0 — see ./reduce.ts) and replays the
// on-disk rollout, whose message schema changed at 0.153 (see ./normalize.ts).
export const codexAdapter: ChatAdapter = {
  id: "codex",
  reduce,
  normalizeTranscript,
  policy: codexPolicy,
  usageEvents,
};
