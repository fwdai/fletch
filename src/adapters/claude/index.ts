import type { ChatAdapter } from "@/adapters/types";
import { normalizeTranscript } from "./normalize";
import { claudePolicy } from "./policy";
import { reduce } from "./reduce";
import { extractUsage } from "./usage";

export const claudeAdapter: ChatAdapter = {
  id: "claude",
  reduce,
  normalizeTranscript,
  policy: claudePolicy,
  extractUsage,
};
