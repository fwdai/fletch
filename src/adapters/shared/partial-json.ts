import { Allow, parse } from "partial-json";

// Partial strings and containers, but NOT partial numbers: a streamed `8` may
// still be growing into `80`, so a number is only trustworthy once a delimiter
// follows it. Strings are the opposite — showing a file path or command fill
// in character by character is exactly what we want.
const ALLOW = Allow.STR | Allow.OBJ | Allow.ARR;

/**
 * Best-effort parse of a JSON document we've only received a prefix of.
 *
 * Claude streams a tool call's input as `input_json_delta` fragments, so at any
 * moment the accumulated text is a partial document — `{"file_path": "/Users/a`.
 * The UI renders tool inputs as objects (a Read row wants `file_path`, not the
 * raw text), so we complete the prefix rather than hand presenters a string.
 *
 * Returns `{}` for input that can't be salvaged — the same shape as the very
 * first fragment, which presenters already render as "no arguments yet".
 */
export function parsePartialJson(raw: string): unknown {
  if (!raw.trim()) return {};
  try {
    return parse(raw, ALLOW);
  } catch {
    return {};
  }
}
