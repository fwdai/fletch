import type { KeyboardEvent } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { Loader } from "@/components/ui/Loader";
import { ScreenshotField } from "./ScreenshotField";
import { FEEDBACK_EMAIL, type FeedbackState } from "./useFeedback";

/** The form body of the feedback modal: message, optional reply-to, optional
 *  screenshot, and the send/failure states. */
export function FeedbackForm({ state, onClose }: { state: FeedbackState; onClose: () => void }) {
  const { message, setMessage, email, setEmail, status, error, canSend, submit } = state;
  const busy = status === "sending";

  if (status === "sent") {
    return (
      <div className="fb-body">
        <div className="fb-done flex-center" role="status">
          <Icon name="check" size={15} />
          <span>Thanks — your feedback is on its way.</span>
        </div>
      </div>
    );
  }

  // ⌘↵ sends, matching the composer's muscle memory. Plain Enter stays a
  // newline: this is a prose box, not a chat input.
  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void submit();
    }
  };

  return (
    <div className="fb-body">
      <div className="fb-field">
        <label className="fb-label text-sm" htmlFor="fb-message">
          What’s on your mind?
        </label>
        <textarea
          id="fb-message"
          className="set-text fb-textarea text-base"
          value={message}
          placeholder="A bug, a rough edge, something you wish Fletch did…"
          autoFocus
          disabled={busy}
          onChange={(e) => setMessage(e.target.value)}
          onKeyDown={onKeyDown}
        />
      </div>

      <div className="fb-field">
        <label className="fb-label text-sm" htmlFor="fb-email">
          Your email <span className="fb-opt">optional, so we can reply</span>
        </label>
        <input
          id="fb-email"
          className="set-text text-base"
          type="email"
          value={email}
          placeholder="ada@example.com"
          spellCheck={false}
          autoComplete="email"
          disabled={busy}
          onChange={(e) => setEmail(e.target.value)}
        />
      </div>

      <ScreenshotField state={state} />

      {status === "error" && (
        <div className="fb-error text-sm">
          <div className="fb-error-t">Couldn’t send that.</div>
          <div>{error}</div>
          <button className="fb-link text-sm" onClick={state.emailInstead}>
            Email {FEEDBACK_EMAIL} instead
          </button>
        </div>
      )}

      <div className="fb-note text-sm">
        Goes straight to the Fletch team, along with your app version and OS — nothing else.
      </div>

      <div className="fb-actions flex-center">
        <Button variant="outline" disabled={busy} onClick={onClose}>
          Cancel
        </Button>
        <Button variant="primary" disabled={!canSend} onClick={() => void submit()}>
          {busy ? <Loader variant="inherit" aria-label="Sending" /> : "Send feedback"}
        </Button>
      </div>
    </div>
  );
}
