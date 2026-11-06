import type { ReactNode } from "react";
import { useState } from "react";
import { PrimaryControl } from "@/components/Composer/PrimaryControl";

/** The PM composer. Reuses the agent composer's skin, but not its input core:
 *  this thread has no attachments, no mentions and no model picker — the agent
 *  is chosen when the chat is created, and everything else here is prose. */
export function Composer({
  disabled,
  placeholder,
  status,
  autoFocus,
  hint,
  onSend,
}: {
  /** The chat can't take a message right now (still loading, or stopped). */
  disabled?: boolean;
  placeholder: string;
  /** Take the caret on mount. Set by the new-chat screen, where typing *is* the
   *  primary action; the live chat leaves focus wherever the user put it. */
  autoFocus?: boolean;
  /** The working strip, which slides up from behind the box — so it is rendered
   *  inside the same `.composer-anchor` the main chat uses, and lines up with
   *  the box rather than with the column. */
  status?: ReactNode;
  /** A line under the box, in the wrap's own column so it inherits the same
   *  centring and gutters. The new-chat screen's keyboard hint lives here. */
  hint?: ReactNode;
  onSend: (text: string) => void;
}) {
  const [draft, setDraft] = useState("");

  const send = () => {
    const text = draft.trim();
    // Mid-turn follow-ups are allowed — the backend injects or queues them —
    // so `disabled` is about readiness, never about the agent being busy.
    if (disabled || !text) return;
    onSend(text);
    setDraft("");
  };

  return (
    <div className="rm-composer-wrap">
      <div className="composer-anchor rm-composer-anchor">
        {status}
        <div className={`composer ${disabled ? "is-blocked" : ""}`}>
          <textarea
            className="composer-input text-base"
            rows={2}
            // The new-chat screen exists to be typed into; the caret starting
            // anywhere else is the bug.
            autoFocus={autoFocus}
            placeholder={placeholder}
            value={draft}
            disabled={disabled}
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
            {/* The agent composer's primary control in its no-dictation form. */}
            <PrimaryControl
              state={draft.trim() ? "draft" : "empty"}
              dictationAvailable={false}
              disabled={disabled}
              onSend={send}
            />
          </div>
        </div>
      </div>
      {hint}
    </div>
  );
}
