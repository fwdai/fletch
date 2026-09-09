import { type FreshSpan, separator } from "./dictation";

export interface GhostProps {
  /** The textarea's own value, repeated invisibly so the interim text lands
   *  exactly where the caret is. */
  text: string;
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
 *  inline at the end of the text, italic, with a pulsing accent dot; they are
 *  not in the textarea's value, so undo history stays clean and a cancel is a
 *  no-op on the draft. After a commit the same layer carries the wash over the
 *  new span. Pointer events pass through to the textarea beneath. */
export function InterimGhost({ text, interim, listening, fresh }: GhostProps) {
  if (fresh) {
    return (
      <div className="cmp-ghost text-base" aria-hidden="true">
        {text.slice(0, fresh.start)}
        <span className="fresh">{text.slice(fresh.start, fresh.end)}</span>
        {text.slice(fresh.end)}
      </div>
    );
  }
  return (
    <div className="cmp-ghost text-base" aria-live="polite">
      {text}
      {separator(text, interim)}
      {interim ? (
        <span className="interim">{interim}</span>
      ) : listening && !text ? (
        <span className="listen-hint">Listening…</span>
      ) : null}
    </div>
  );
}
