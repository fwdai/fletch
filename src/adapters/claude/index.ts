import type { ChatAdapter } from "../types";
import { reduce } from "./reduce";
import { normalizeTranscript } from "./normalize";
import { claudePolicy } from "./policy";
import { extractUsage } from "./usage";

export const claudeAdapter: ChatAdapter = {
  id: "claude",
  reduce,
  normalizeTranscript,
  policy: claudePolicy,
  extractUsage,
};
