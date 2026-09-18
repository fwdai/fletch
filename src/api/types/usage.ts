/** Token-usage DTOs for the whole-disk transcript scan (`scan_usage_transcripts`).
 *  Counts only — the backend does no pricing; the usage page prices these
 *  buckets against the models.dev catalog. Buckets are hourly and sessions
 *  carry their span, so one scan of the widest window can be re-sliced into any
 *  shorter range client-side instead of re-walking the disk. */

/** Agent CLI a usage bucket came from. */
export type UsageProvider = "claude" | "codex";

/** Token counts for one bucket. `input` is *fresh* (uncached) input only, so
 *  the four fields are disjoint and each is priced at its own rate. */
export interface UsageScanTokens {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

/** One hour/provider/model cell of the usage table. */
export interface UsageBucket {
  /** Epoch ms of the *local* hour containing the records. */
  hourStartMs: number;
  provider: UsageProvider;
  model: string;
  tokens: UsageScanTokens;
  /** Usage records folded into this bucket, after dedupe. */
  requests: number;
}

/** One distinct session that contributed records, and the span of those
 *  records — count a sub-range's sessions by keeping the spans overlapping it. */
export interface UsageSessionSpan {
  provider: UsageProvider;
  id: string;
  firstMs: number;
  lastMs: number;
}

export interface UsageScan {
  /** Sorted by hour, then provider, then model. */
  buckets: UsageBucket[];
  /** Sorted by provider, then start, then id. */
  sessions: UsageSessionSpan[];
  /** Transcript files whose records went into this answer — read now or reused
   *  from the backend's in-memory cache. Files dropped by the mtime prefilter
   *  are not counted. */
  scannedFiles: number;
  /** Of those, the files this scan actually opened. The backend caches parsed
   *  records per file for the process's lifetime, so a refresh only reads files
   *  that changed, and only their appended tail: on an idle machine this is 0.
   *  Observability only — nothing renders it yet. */
  filesRead: number;
  /** Bytes this scan pulled off disk. Observability only. */
  bytesRead: number;
  /** Echo of the requested window, to check a cached scan covers the range
   *  being sliced out of it. */
  sinceMs: number;
  untilMs: number;
}
