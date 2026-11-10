import { FletchMark } from "@/components/FletchMark";
import "./FletchLockup.css";

/** The brand lockup: the bolt mark and the lowercase wordmark as one tight
 *  unit. It scales from the parent's font-size and takes its color, so the
 *  title bar, onboarding and any future surface draw the same lockup at
 *  their own size. */
export function FletchLockup({ className }: { className?: string }) {
  return (
    <span className={["lockup", className].filter(Boolean).join(" ")} aria-label="Fletch">
      <FletchMark className="lockup-mark" />
      <span className="lockup-word" aria-hidden="true">
        fletch
      </span>
    </span>
  );
}
