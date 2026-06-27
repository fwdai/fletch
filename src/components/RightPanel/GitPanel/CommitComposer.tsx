import type { RefObject } from "react";
import { Icon } from "../../Icon";

// ── Commit message composer ───────────────────────────────────────
// Lives in one fixed slot directly above the status + action. By default
// (agent mode) it's collapsed to a quiet one-liner explaining the agent will
// write the message + PR, with an inline "Write it yourself" opt-in. Clicking
// it expands a textarea IN PLACE — the note collapses and the field grows in a
// single smooth animation (CSS grid-rows). Typing makes a direct commit that
// bypasses the agent; "Let agent write it" collapses back.
export function CommitComposer({
  writing,
  msg,
  setMsg,
  textareaRef,
  onOpen,
  onRevert,
  onSubmit,
}: {
  writing: boolean;
  msg: string;
  setMsg: (v: string) => void;
  textareaRef: RefObject<HTMLTextAreaElement>;
  onOpen: () => void;
  onRevert: () => void;
  onSubmit: () => void;
}) {
  const hasMsg = msg.trim().length > 0;
  return (
    <div className="git-commit">
      {/* collapsed note — animates shut when writing */}
      <div className={`cm-row note ${writing ? "shut" : ""}`} aria-hidden={writing}>
        <div className="cm-row-inner">
          <div className="cm-note">
            Agent will write the commit message &amp; PR.{" "}
            <button className="cm-link" onClick={onOpen} tabIndex={writing ? -1 : 0}>
              Write it yourself
            </button>
          </div>
        </div>
      </div>

      {/* override field — animates open when writing */}
      <div className={`cm-row field ${writing ? "open" : ""}`} aria-hidden={!writing}>
        <div className="cm-row-inner">
          <div className="cm-title">
            <span>Your message</span>
            <span className="grow" />
            <button className="cm-revert" onClick={onRevert} tabIndex={writing ? 0 : -1}>
              <Icon name="close" size={11} />
              <span>Let agent write it</span>
            </button>
          </div>
          <textarea
            ref={textareaRef}
            className="cm-input"
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
          <div className={`cm-foot ${hasMsg ? "on" : ""}`}>
            {hasMsg ? (
              <>
                <Icon name="branch" size={11} />
                <span>Commits directly with your message — the agent is skipped.</span>
              </>
            ) : (
              <span className="cm-foot-dim">Leave empty to let the agent write it.</span>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
