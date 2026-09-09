import { type FreshSpan, splitForTranscript } from "./dictation";

export interface GhostProps {
  /** The textarea's own value, repeated invisibly so the interim text lands
   *  exactly where it will be inserted. */
  text: string;
  /** Where the transcript will be inserted — the textarea's caret. */
  caret: number;
  /** The recognizer's running transcript, not yet in the box. */
  interim: string;
  /** The mic is open. With nothing typed and nothing heard yet, the layer says
   *  so in place of the placeholder. */
  listening: boolean;
  /** The span the last commit inserted, marked for a moment so the eye can
   *  find what just arrived. */
  fresh: FreshSpan | null;
}

/** A layer over the textarea with identical metrics. Interim tokens render
 *  inline at the caret, italic, with a pulsing accent dot — spaced exactly as
 *  the commit will space them; they are not in the textarea's value, so undo
 *  history stays clean and a cancel is a no-op on the draft. After a commit the
 *  same layer carries the wash over the new span. Pointer events pass through
 *  to the textarea beneath. */
export function InterimGhost({ text, caret, interim, listening, fresh }: GhostProps) {
  if (fresh) {
    return (
      <div className="cmp-ghost text-base" aria-hidden="true">
        {text.slice(0, fresh.start)}
        <span className="fresh">{text.slice(fresh.start, fresh.end)}</span>
        {text.slice(fresh.end)}
      </div>
    );
  }
  const { before, lead, trail, after } = splitForTranscript(text, caret, interim);
  return (
    <div className="cmp-ghost text-base" aria-live="polite">
      {before}
      {lead}
      {interim ? (
        <span className="interim">{interim}</span>
      ) : listening && !text ? (
        <span className="listen-hint">Listening…</span>
      ) : null}
      {trail}
      {after}
    </div>
  );
}
