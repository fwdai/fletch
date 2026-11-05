import { Icon } from "@/components/Icon";

/** The shrink segment attached to the chip's right edge. Refused (with why, in
 *  its tooltip) rather than hidden while a turn runs or the agent isn't ready,
 *  so the control doesn't jump in and out with every turn. `aria-disabled`, not the
 *  native attribute: a disabled <button> swallows hover in the WebView, and
 *  its tooltip would never show. */
export function CompactButton({
  blocker,
  keys,
  onCompact,
}: {
  blocker: string | null;
  keys: string;
  onCompact: () => void;
}) {
  const tip = blocker ?? (keys ? `Compact conversation (${keys})` : "Compact conversation");
  return (
    <button
      type="button"
      className="husage-act tip"
      data-tip={tip}
      aria-label="Compact conversation"
      aria-disabled={blocker != null}
      onClick={() => {
        if (!blocker) onCompact();
      }}
    >
      <Icon name="compact" size={13} strokeWidth={1.75} />
    </button>
  );
}
