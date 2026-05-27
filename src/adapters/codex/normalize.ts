// TODO(codex-real-impl): codex transcript format isn't yet verified.
// This normalizer assumes JSONL records carrying the same `type` strings
// the reducer recognizes — pass them through unchanged. Real transcripts
// may need translation when the codex CLI's on-disk format is known.

import type { RawEvent } from "../types";
import { asRecord } from "../shared/json";

const PASS_THROUGH = new Set([
  "user",
  "message",
  "function_call",
  "function_call_output",
  "result",
]);

export function normalizeTranscript(lines: unknown[]): RawEvent[] {
  const out: RawEvent[] = [];
  for (const raw of lines) {
    const rec = asRecord(raw);
    const type = typeof rec.type === "string" ? rec.type : undefined;
    if (type && PASS_THROUGH.has(type)) out.push(rec as RawEvent);
  }
  return out;
}
