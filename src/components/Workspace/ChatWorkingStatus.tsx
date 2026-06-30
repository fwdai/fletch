import { useEffect, useState } from "react";
import { Icon } from "../Icon";
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

/** Pinned working indicator — slides up from behind the composer on show,
 *  back down on turn end. */
export function ChatWorkingStatus({
  visible,
  label,
  startedAt,
}: {
  visible: boolean;
  label: string;
  /** Epoch millis the open turn started; when set, a live timer ticks. */
  startedAt?: number;
}) {
  const { mounted, open } = useSlideReveal(visible);
  if (!mounted) return null;

  return (
    <div className={`chat-status${open ? " is-open" : ""}`} role="status" aria-live="polite">
      <span className="dots">
        <i />
        <i />
        <i />
      </span>
      <span className="chat-status-label">{label}</span>
      {startedAt != null && (
        <>
          <span className="chat-status-sep">·</span>
          <Icon name="clock" size={11} className="turn-clock-i" />
          <LiveTimer startedAt={startedAt} />
        </>
      )}
    </div>
  );
}
