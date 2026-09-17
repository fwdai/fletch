import type { RefObject } from "react";
import { Icon } from "@/components/Icon";

// ── Commit message composer ───────────────────────────────────────
// Two pieces that share one slot in the footer:
//
//   CommitStatus  — the one-line note in the action bar's status slot. By
//                   default (agent mode) it says the agent will write the
//                   message, with an inline "Write it yourself" opt-in; while
//                   the field is open it carries the field's hint instead.
//   CommitComposer — the textarea, which animates open IN PLACE above the
//                   action bar (CSS grid-rows) when the user opts in. Typing
//                   makes a direct commit that bypasses the agent; "Let agent
//                   write it" collapses it back.
//
// Splitting the note out of the composer is what keeps the footer one row: the
// status slot already exists, and "who writes the message" is the changes
// state's status.

/** The changes-state status line: who is writing the commit message, and how
 *  to change that. Replaces the old "Ready to commit · N files", which only
 *  repeated the header and the button. */
export function CommitStatus({
  writing,
  hasMsg,
  onOpen,
}: {
  writing: boolean;
  hasMsg: boolean;
  onOpen: () => void;
}) {
  if (!writing) {
    return (
      <>
        Agent writes the commit message ·{" "}
        <button type="button" className="cm-link" onClick={onOpen}>
          Write it yourself
        </button>
      </>
    );
  }
  // The field is open: the hint that used to sit under the textarea.
  return hasMsg ? (
    <>Commits directly with your message — the agent is skipped</>
  ) : (
    <>Leave empty to let the agent write it</>
  );
}

export function CommitComposer({
  writing,
  msg,
  setMsg,
  textareaRef,
  onRevert,
  onSubmit,
}: {
  writing: boolean;
  msg: string;
  setMsg: (v: string) => void;
  textareaRef: RefObject<HTMLTextAreaElement>;
  onRevert: () => void;
  onSubmit: () => void;
}) {
  return (
    <div className="git-commit">
      {/* Always mounted so the open/close animates; closed it takes no space
          (the row's padding lives on the inner, inside the 0fr track). */}
      <div className={`cm-row field ${writing ? "open" : ""}`} aria-hidden={!writing}>
        <div className="cm-row-inner">
          <div className="cm-title text-xs">
            <span>Your message</span>
            <span className="grow" />
            <button className="cm-revert text-xs" onClick={onRevert} tabIndex={writing ? 0 : -1}>
              <Icon name="close" size={11} />
              <span>Let agent write it</span>
            </button>
          </div>
          <textarea
            ref={textareaRef}
            className="cm-input text-sm"
            rows={2}
            placeholder="Describe this commit…"
            value={msg}
            tabIndex={writing ? 0 : -1}
            onChange={(e) => setMsg(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                onSubmit();
              }
            }}
          />
        </div>
      </div>
    </div>
  );
}
