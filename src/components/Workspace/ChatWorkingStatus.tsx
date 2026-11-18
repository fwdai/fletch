import { useEffect, useState } from "react";
import { Icon } from "@/components/Icon";
import { LiveTimer } from "./RunTimer";

const DURATION_MS = 130;

function useSlideReveal(visible: boolean) {
  const [mounted, setMounted] = useState(visible);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (visible) {
      setMounted(true);
      let id1 = 0;
      let id2 = 0;
      id1 = requestAnimationFrame(() => {
        id2 = requestAnimationFrame(() => setOpen(true));
      });
      return () => {
        cancelAnimationFrame(id1);
        cancelAnimationFrame(id2);
      };
    }
    setOpen(false);
    const t = window.setTimeout(() => setMounted(false), DURATION_MS);
    return () => clearTimeout(t);
  }, [visible]);

  return { mounted, open };
}

/** Something the running turn holds back, with a way to stop waiting for it
 *  (a granted folder that applies when the turn ends). */
export interface WorkingNotice {
  text: string;
  action: string;
  /** Tooltip for the action: what taking it does to the running turn. */
  actionTip: string;
  onAction: () => void;
}

/** The open turn's status by the composer: what the agent is doing and for how
 *  long, as "Working · 12s" behind the clock that leads the "Ran 38s" a
 *  finished turn leaves in the feed. Slides up from behind the composer on
 *  show, back down on turn end.
 *
 *  One line, one look, from the send to the turn's end. The count is from the
 *  host's start of the turn, which is the moment the message was accepted;
 *  it joins the line when the host has said so (at once, but for a host from
 *  before open turns, which only says when the agent starts running). The
 *  composer's own control already shows a run in progress, so the line is the
 *  datum, not a second spinner. */
export function ChatWorkingStatus({
  visible,
  label,
  startedAt,
  notice,
}: {
  visible: boolean;
  /** The activity as one word: "Working", "Compacting". */
  label: string;
  /** Epoch millis the open turn started; when set, a live timer ticks. */
  startedAt?: number;
  notice?: WorkingNotice;
}) {
  const { mounted, open } = useSlideReveal(visible);
  if (!mounted) return null;

  return (
    <div className={`chat-status${open ? " is-open" : ""}`} role="status" aria-live="polite">
      <Icon name="clock" size={11} className="turn-clock-i" />
      <span className="chat-status-label">{label}</span>
      {startedAt != null && (
        <>
          <span className="chat-status-sep">·</span>
          <LiveTimer startedAt={startedAt} />
        </>
      )}
      {notice && (
        <>
          <span className="chat-status-sep">·</span>
          <span className="chat-status-note">{notice.text}</span>
          <button
            type="button"
            className="chat-status-action"
            title={notice.actionTip}
            onClick={notice.onAction}
          >
            {notice.action}
          </button>
        </>
      )}
    </div>
  );
}
