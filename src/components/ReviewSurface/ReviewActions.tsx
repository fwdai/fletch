// ReviewSurface/ReviewActions — the Approve / Request-changes footer. Reject
// requires a note (a textarea), and the consequence is stated plainly: the
// step's agent gets one more attempt within budget. Owns its own busy + note
// state so the surface can stay a pure view.

import { useEffect, useRef, useState } from "react";
import { Icon } from "../Icon";
import { Button } from "../ui";

export function ReviewActions({
  onApprove,
  onReject,
  onError,
}: {
  onApprove: () => Promise<void>;
  onReject: (note: string) => Promise<void>;
  onError?: (message: string) => void;
}) {
  const [rejecting, setRejecting] = useState(false);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const noteRef = useRef<HTMLTextAreaElement>(null);

  // Focus the note the moment the reject form opens — it's the sole task now.
  useEffect(() => {
    if (rejecting) noteRef.current?.focus();
  }, [rejecting]);

  const run = async (action: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    try {
      await action();
      // On success the run row flips over the `wf:run` subscription; the parent
      // closes the surface. Nothing to do here.
    } catch (e) {
      onError?.(`Action failed: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const submitReject = () => {
    const trimmed = note.trim();
    if (!trimmed || busy) return;
    void run(() => onReject(trimmed));
  };

  if (rejecting) {
    return (
      <div className="rv-reject">
        <label className="rv-reject-label" htmlFor="rv-reject-note">
          Request changes
        </label>
        <textarea
          id="rv-reject-note"
          ref={noteRef}
          className="rv-reject-input"
          placeholder="What needs to change before this can be approved? Be specific — this note is sent straight to the agent."
          value={note}
          disabled={busy}
          onChange={(e) => setNote(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              submitReject();
            }
          }}
        />
        <div className="rv-reject-row">
          <span className="rv-reject-hint">
            The step's agent gets one more attempt within budget.
          </span>
          <div className="rv-reject-btns">
            <Button variant="ghost" disabled={busy} onClick={() => setRejecting(false)}>
              Cancel
            </Button>
            <Button variant="outline" danger disabled={busy || !note.trim()} onClick={submitReject}>
              <Icon name="close" size={13} /> Send rejection
            </Button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="rv-actions-row">
      <Button variant="outline" danger disabled={busy} onClick={() => setRejecting(true)}>
        <Icon name="close" size={13} /> Request changes
      </Button>
      <Button variant="primary" disabled={busy} onClick={() => void run(onApprove)}>
        <Icon name="check" size={13} /> Approve
      </Button>
    </div>
  );
}
