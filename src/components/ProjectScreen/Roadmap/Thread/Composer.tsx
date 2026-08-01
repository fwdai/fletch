import { useState } from "react";
import { Icon } from "@/components/Icon";
import { Chip } from "@/components/ui/Chip";

/** The PM composer. Reuses the agent composer's skin, but not its input core —
 *  there are no mentions, attachments or model picker to wire up here. Closed
 *  while a proposal or question is waiting on the user, so the board can't
 *  drift behind an unanswered decision. */
export function Composer({
  blocked,
  suggestions,
  onSend,
}: {
  blocked: boolean;
  /** Unplayed openers, offered as dashed chips above the box. */
  suggestions: string[];
  onSend: (text: string) => void;
}) {
  const [draft, setDraft] = useState("");

  const send = () => {
    if (blocked || !draft.trim()) return;
    onSend(draft);
    setDraft("");
  };

  return (
    <div className="rm-composer-wrap">
      {!blocked && suggestions.length > 0 && (
        <div className="rm-sugg">
          {suggestions.map((s) => (
            <button key={s} type="button" className="rm-sugg-c text-sm" onClick={() => onSend(s)}>
              {s}
            </button>
          ))}
        </div>
      )}

      <div className={`composer ${blocked ? "is-blocked" : ""}`}>
        <textarea
          className="composer-input text-base"
          rows={2}
          placeholder={
            blocked
              ? "Resolve the proposal above to keep going…"
              : "Tell the PM what the product should do…"
          }
          value={draft}
          disabled={blocked}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            }
          }}
        />
        <div className="composer-foot flex-center">
          <Chip disabled={blocked}>
            <Icon name="attach" size={12} /> Attach
          </Chip>
          <span className="grow" />
          <button
            type="button"
            className="send flex-center"
            disabled={blocked || !draft.trim()}
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
