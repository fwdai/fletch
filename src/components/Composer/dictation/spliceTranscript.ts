/** Offsets into the composer text of the span the last commit inserted — what
 *  the ghost layer marks for a moment so the eye can find what just arrived. */
export interface FreshSpan {
  start: number;
  end: number;
}

export interface SplicedTranscript {
  text: string;
  /** Offset where the transcript begins in `text` — the span to mark as fresh. */
  start: number;
  /** Caret offset after the splice — the end of the transcript, so typing
   *  continues right after the dictated words. */
  caret: number;
}

/** The word boundary between `base` and a transcript that follows it: one
 *  space, unless there is nothing to join or the user already left one.
 *  Keeping their trailing space or newline intact matters for a dictated list
 *  item. */
export function separator(base: string, transcript: string): string {
  if (!transcript || !base || /\s$/.test(base)) return "";
  return " ";
}

/** The pieces of inserting `transcript` into `text` at `caret`: the text either
 *  side, and the one space added on each side where a word boundary is missing.
 *  Shared by the splice itself and by the ghost layer, which previews exactly
 *  this before it lands. */
export interface TranscriptSplit {
  before: string;
  lead: string;
  trail: string;
  after: string;
}

export function splitForTranscript(
  text: string,
  caret: number,
  transcript: string,
): TranscriptSplit {
  const at = Math.max(0, Math.min(caret, text.length));
  const before = text.slice(0, at);
  const after = text.slice(at);
  return {
    before,
    lead: separator(before, transcript),
    trail: transcript && after && !/^\s/.test(after) ? " " : "",
    after,
  };
}

/** Insert a transcript into the composer text at the caret, space-normalised
 *  on both sides. Nothing heard leaves the text untouched — no dangling space.
 *
 *  Apple's recognizer revises earlier words as it hears more, so the session's
 *  transcript is one whole text, not a delta — the caller shows it beside the
 *  box while it changes and inserts it once, at the end. This is the one place
 *  that decides how the pieces are joined. */
export function insertTranscript(
  text: string,
  caret: number,
  transcript: string,
): SplicedTranscript {
  const { before, lead, trail, after } = splitForTranscript(text, caret, transcript);
  if (!transcript) return { text, start: before.length, caret: before.length };
  const start = before.length + lead.length;
  const end = start + transcript.length;
  return { text: before + lead + transcript + trail + after, start, caret: end };
}

/** [`insertTranscript`] at the end of `base` — for a composer with no caret to
 *  speak of (the phone's, where the transcript arrives once, after the fact). */
export function spliceTranscript(base: string, transcript: string): SplicedTranscript {
  return insertTranscript(base, base.length, transcript);
}
