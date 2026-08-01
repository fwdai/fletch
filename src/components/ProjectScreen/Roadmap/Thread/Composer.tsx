import { useState } from "react";
import { Icon } from "@/components/Icon";

/** The PM composer. Reuses the agent composer's skin, but not its input core —
 *  there are no mentions, attachments or model picker to wire up here.
 *
 *  Closed while the PM is mid-reply (`busy`) so a second message can't
 *  interleave its beats with the first, and while a proposal or question is
 *  waiting on the user (`blocked`) so the board can't drift behind an
 *  unresolved decision. */
export function Composer({
  blocked,
  busy,
  suggestions,
  onSend,
}: {
  /** A proposal or question is waiting on the user. */
  blocked: boolean;
  /** A reply is still landing. */
  busy: boolean;
  /** Unplayed openers, offered as dashed chips above the box. */
  suggestions: string[];
  onSend: (text: string) => void;
}) {
  const [draft, setDraft] = useState("");
  const closed = blocked || busy;

  const send = () => {
    if (closed || !draft.trim()) return;
    onSend(draft);
    setDraft("");
  };

  return (
    <div className="rm-composer-wrap">
      {!closed && suggestions.length > 0 && (
        <div className="rm-sugg">
          {suggestions.map((s) => (
            <button key={s} type="button" className="rm-sugg-c text-sm" onClick={() => onSend(s)}>
              {s}
            </button>
          ))}
        </div>
      )}

      <div className={`composer ${closed ? "is-blocked" : ""}`}>
        <textarea
          className="composer-input text-base"
          rows={2}
          placeholder={
            blocked
              ? "Resolve the proposal above to keep going…"
              : busy
                ? "The PM is working through it…"
                : "Tell the PM what the product should do…"
          }
          value={draft}
          disabled={closed}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            }
          }}
        />
        <div className="composer-foot flex-center">
          <span className="grow" />
          <button
            type="button"
            className="send flex-center"
            disabled={closed || !draft.trim()}
            onClick={send}
            aria-label="Send"
          >
            <Icon name="arrowUp" size={13} />
          </button>
        </div>
      </div>
    </div>
  );
}
