import { invoke } from "../invoke";
import type { UsageScan } from "../types/usage";

export const usageApi = {
  /** Scan every Claude Code and Codex transcript on disk (not just sessions
   *  this app spawned) and return token counts bucketed by local hour, provider
   *  and model over `[sinceMs, untilMs)`, plus a span per contributing session.
   *  Uncached and slow (seconds over a 90-day window): scan the widest range
   *  once and slice shorter ranges from the result rather than calling again. */
  scanUsageTranscripts: (sinceMs: number, untilMs: number) =>
    invoke<UsageScan>("scan_usage_transcripts", { sinceMs, untilMs }),
};
