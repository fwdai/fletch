import { invokeLocal } from "../invoke";
import type { UsageScan } from "../types/usage";

export const usageApi = {
  /** Scan every Claude Code and Codex transcript on *this machine's* disk (not
   *  just sessions this app spawned) and return token counts bucketed by local
   *  hour, provider and model over `[sinceMs, untilMs)`, plus a span per
   *  contributing session. Uncached and slow (seconds over a 90-day window):
   *  scan the widest range once and slice shorter ranges from the result rather
   *  than calling again.
   *
   *  Deliberately `invokeLocal`: a scan is a fact about one machine's disk, so
   *  it must not follow the active environment. The Usage pane asks every
   *  connected host for its own scan and merges them — see
   *  `src/data/usage/useUsageStats.ts`, which calls a remote host's transport
   *  directly for the same op. */
  scanUsageTranscripts: (sinceMs: number, untilMs: number) =>
    invokeLocal<UsageScan>("scan_usage_transcripts", { sinceMs, untilMs }),
};
