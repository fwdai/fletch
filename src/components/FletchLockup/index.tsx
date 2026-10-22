import type { ReactNode } from "react";
import { FletchMark } from "@/components/FletchMark";
import "./FletchLockup.css";

/** The brand lockup: the bolt mark and the lowercase wordmark as one tight
 *  unit. It scales from the parent's font-size and takes its color, so the
 *  title bar, onboarding and any future surface draw the same lockup at
 *  their own size. `children` land after the wordmark, for a meta tag such
 *  as the title bar's "beta". */
export function FletchLockup({
  className,
  children,
}: {
  className?: string;
  children?: ReactNode;
}) {
  return (
    <span className={["lockup", className].filter(Boolean).join(" ")} aria-label="Fletch">
      <FletchMark className="lockup-mark" />
      <span className="lockup-word" aria-hidden="true">
        fletch
      </span>
      {children}
    </span>
  );
}
