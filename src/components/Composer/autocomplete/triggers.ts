// Pure detection of an autocomplete trigger token under the caret, shared by
// every source. A token is the trigger char plus following non-whitespace,
// non-trigger characters (so `@`, `#`, `/` each delimit the next).

const TOKEN_CHARS = /[^\s@#]/;

function escape(ch: string): string {
  return ch.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** The trigger token ending at the caret, or null. `start` is the index of
 *  the trigger char. The trigger must sit at the start of the text/line or
 *  after whitespace (`lineStart` restricts it to line starts, for `/`), so
 *  things like `foo@bar` or a mid-prose `#` after a word don't fire. */
export function triggerQueryAt(
  text: string,
  caret: number,
  trigger: string,
  lineStart = false,
): { query: string; start: number } | null {
  const upto = text.slice(0, caret);
  const anchor = lineStart ? "(?:^|\\n)" : "(?:^|\\s)";
  const re = new RegExp(`${anchor}${escape(trigger)}([^\\s@#]*)$`);
  const m = re.exec(upto);
  if (!m) return null;
  return { query: m[1], start: caret - m[1].length - trigger.length };
}

/** Index where the token under the caret ends — the first whitespace or
 *  trigger char at or after `from`, else the end of the text. Lets a pick
 *  replace the whole token even when the caret was moved back into it. */
export function triggerTokenEnd(text: string, from: number): number {
  let end = from;
  while (end < text.length && TOKEN_CHARS.test(text[end])) end++;
  return end;
}
