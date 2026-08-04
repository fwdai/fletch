// The item form's dependency chips, as pure functions.
//
// `deps` is a list of item *codes* ("FLT-142") this item must land after — the
// same field the PM writes and the queue drainer reads (src-tauri/src/roadmap).
// The rules here are the fast half of the answer: they refuse a code that isn't
// on the board, a duplicate, and self-reference before a round trip. They
// deliberately do NOT know about loops — that is a graph question, and the
// backend's `roadmap::deps` module is the authority for it (a loop refusal comes
// back as an error the dialog renders, naming the loop). Two implementations of
// cycle detection would be one too many.
//
// Split out of the dialog because it is exactly the part worth testing: a chip
// field is markup, but "which token becomes which code" is a rule.

/** Nothing to do (the box was empty), a new list, or a refusal — exactly one. */
export interface DepAdd {
  /** The list to keep, when the token resolved. Null when nothing changed. */
  deps: string[] | null;
  /** Why the token was refused, for the dialog's error slot. */
  error: string | null;
}

/** The code a typed token means. Codes are upper case by construction
 *  (`store::code_prefix`), so "flt-142" is the same item — but an exact match
 *  always wins, because the prefix is per project and this must not invent one. */
export function resolveCode(raw: string, codes: ReadonlySet<string>): string {
  const trimmed = raw.trim();
  if (codes.has(trimmed)) return trimmed;
  const upper = trimmed.toUpperCase();
  return codes.has(upper) ? upper : trimmed;
}

/** Add a typed token to a dep list, or say why not. Idempotent on a code that
 *  is already there: re-typing a dep is not an error worth a red line. */
export function addDep(
  current: readonly string[],
  raw: string,
  codes: ReadonlySet<string>,
  self?: string | null,
): DepAdd {
  if (!raw.trim()) return { deps: null, error: null };
  const code = resolveCode(raw, codes);
  if (self && code === self) {
    return { deps: null, error: `${code} can't depend on itself.` };
  }
  if (!codes.has(code)) {
    return { deps: null, error: `There is no ${code} on this board.` };
  }
  if (current.includes(code)) return { deps: [...current], error: null };
  return { deps: [...current, code], error: null };
}

/** Drop a chip. */
export function removeDep(current: readonly string[], code: string): string[] {
  return current.filter((c) => c !== code);
}

/** The codes to offer for what has been typed so far: every code on the board
 *  that isn't this item, isn't already a chip, and contains the query (so
 *  "142" finds "FLT-142" — the prefix is the same for every row and typing it
 *  is pure ceremony). Sorted, and capped so the list stays glanceable. */
export function suggestCodes(
  query: string,
  codes: ReadonlySet<string>,
  chosen: readonly string[],
  self?: string | null,
  limit = 8,
): string[] {
  const q = query.trim().toUpperCase();
  const taken = new Set(chosen);
  return [...codes]
    .filter((c) => c !== self && !taken.has(c) && (!q || c.toUpperCase().includes(q)))
    .sort()
    .slice(0, limit);
}
