export interface SplicedTranscript {
  text: string;
  /** Caret offset after the splice — always the end, so typing continues after
   *  the dictated words. */
  caret: number;
}

/** Where the running transcript lands in the composer, given `base` (the text
 *  that was in the box when the session started).
 *
 *  Apple's recognizer revises earlier words as it hears more, so each event
 *  carries the WHOLE transcript of the session, not a delta — the caller
 *  replaces everything after `base` on every event instead of appending, and
 *  this function is the one place that decides how the two are joined. */
export function spliceTranscript(base: string, transcript: string): SplicedTranscript {
  let text: string;
  // A word boundary is only ours to add when the user didn't leave one: keeping
  // their trailing space or newline intact matters for a dictated list item.
  if (!base) text = transcript;
  else if (/\s$/.test(base)) text = base + transcript;
  else text = `${base} ${transcript}`;
  return { text, caret: text.length };
}
