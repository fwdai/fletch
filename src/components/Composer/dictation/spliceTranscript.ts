export interface SplicedTranscript {
  text: string;
  /** Caret offset after the splice — always the end, so typing continues after
   *  the dictated words. */
  caret: number;
}

/** The word boundary between `base` and a transcript that follows it: one
 *  space, unless there is nothing to join or the user already left one.
 *  Keeping their trailing space or newline intact matters for a dictated list
 *  item. Shared with the ghost layer, which previews the join before it lands. */
export function separator(base: string, transcript: string): string {
  if (!transcript || !base || /\s$/.test(base)) return "";
  return " ";
}

/** Where a transcript lands in the composer, given `base` (the text in the box
 *  when it arrives).
 *
 *  Apple's recognizer revises earlier words as it hears more, so the session's
 *  transcript is one whole text, not a delta — the caller shows it beside the
 *  box while it changes and splices it once, at the end. This function is the
 *  one place that decides how the two are joined. Nothing heard leaves the base
 *  untouched — no dangling space. */
export function spliceTranscript(base: string, transcript: string): SplicedTranscript {
  const text = base + separator(base, transcript) + transcript;
  return { text, caret: text.length };
}
