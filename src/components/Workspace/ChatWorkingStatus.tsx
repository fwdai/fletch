import { useEffect, useState } from "react";

const DURATION_MS = 130;

function useSlideReveal(visible: boolean) {
  const [mounted, setMounted] = useState(visible);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (visible) {
      setMounted(true);
      const id = requestAnimationFrame(() => {
        requestAnimationFrame(() => setOpen(true));
      });
      return () => cancelAnimationFrame(id);
    }
    setOpen(false);
    const t = window.setTimeout(() => setMounted(false), DURATION_MS);
    return () => clearTimeout(t);
  }, [visible]);

  return { mounted, open };
}

/** Pinned working indicator — slides up from behind the composer on show,
 *  back down on turn end. */
export function ChatWorkingStatus({ visible, label }: { visible: boolean; label: string }) {
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
    </div>
  );
}
