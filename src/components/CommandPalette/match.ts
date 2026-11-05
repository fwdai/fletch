// Ranking for the command palette. One query, many candidates, and a score
// that prefers what people mean: an exact title, then a title that starts
// with the query, then one with a word that does, then the query anywhere
// in it, and only then the loose subsequence a fuzzy finder accepts. Pure,
// so the ordering is testable without a store or a DOM.

export interface Match {
  score: number;
  /** Positions in the candidate that the query's characters landed on, for
   *  highlighting. Empty for a keyword-only match. */
  indices: number[];
}

const EXACT = 1000;
const PREFIX = 800;
const WORD = 600;
const SUBSTRING = 400;

/** Whether position `i` of `text` starts a word: the first character, or one
 *  after a space or punctuation. */
function wordStart(text: string, i: number): boolean {
  return i === 0 || /[\s\-_/.:›·(]/.test(text[i - 1]);
}

/** The loose match: every query character in order, anywhere in `text`.
 *  Scored for runs (consecutive characters) and word starts, penalised for
 *  gaps, and always below `SUBSTRING` so it never outranks a literal hit. */
function subsequence(query: string, text: string): Match | null {
  const indices: number[] = [];
  let score = 0;
  let at = 0;
  for (const ch of query) {
    const i = text.indexOf(ch, at);
    if (i === -1) return null;
    score += 10;
    if (wordStart(text, i)) score += 15;
    if (indices.length > 0 && i === indices[indices.length - 1] + 1) score += 8;
    score -= Math.min(i - at, 20);
    indices.push(i);
    at = i + 1;
  }
  return { score: Math.max(1, Math.min(score, SUBSTRING - 1)), indices };
}

/** Score `query` against `text`, or null when it does not match at all. The
 *  empty query matches everything at zero, so an unfiltered list keeps its
 *  own order. Case-insensitive; a longer candidate scores a little lower than
 *  a shorter one at the same tier, so "Git panel" beats "Project settings"
 *  for "p" only when the match is of the same kind. */
export function fuzzyMatch(query: string, text: string): Match | null {
  const q = query.trim().toLowerCase();
  if (!q) return { score: 0, indices: [] };
  const t = text.toLowerCase();
  const range = (from: number) => Array.from({ length: q.length }, (_, k) => from + k);
  if (t === q) return { score: EXACT, indices: range(0) };
  if (t.startsWith(q)) return { score: PREFIX - Math.min(t.length, 40) / 10, indices: range(0) };
  const first = t.indexOf(q);
  if (first === -1) return subsequence(q, t);
  for (let i = first; i !== -1; i = t.indexOf(q, i + 1)) {
    if (wordStart(t, i)) return { score: WORD - i / 10, indices: range(i) };
  }
  return { score: SUBSTRING - first / 10, indices: range(first) };
}

/** The best of `label` and each keyword. A keyword can only ever lift a row
 *  to just under the literal label tiers: a label that contains the query
 *  always beats a row that merely lists it among its synonyms, so "Ocean"
 *  the accent outranks a row with ocean in its keywords. The label's own
 *  highlight is kept whenever the label matched at all. */
export function bestMatch(query: string, label: string, keywords: string = ""): Match | null {
  const own = fuzzyMatch(query, label);
  if (!query.trim() || !keywords) return own;
  let best = own;
  for (const kw of keywords.split(/\s{2,}|\n/)) {
    const m = fuzzyMatch(query, kw);
    if (!m) continue;
    const score = Math.min(m.score, SUBSTRING - 1) - 10;
    if (!best || score > best.score) best = { score, indices: own?.indices ?? [] };
  }
  return best;
}

/** Split `text` into runs for rendering: `[piece, highlighted]`. */
export function highlightRuns(text: string, indices: number[]): [string, boolean][] {
  if (indices.length === 0) return [[text, false]];
  const set = new Set(indices);
  const runs: [string, boolean][] = [];
  let buf = "";
  let hl = set.has(0);
  for (let i = 0; i < text.length; i++) {
    const on = set.has(i);
    if (on !== hl) {
      if (buf) runs.push([buf, hl]);
      buf = "";
      hl = on;
    }
    buf += text[i];
  }
  if (buf) runs.push([buf, hl]);
  return runs;
}
