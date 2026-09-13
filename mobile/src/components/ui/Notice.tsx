import { Icon, type IconName } from "@desktop/components/Icon";
import type { ReactNode } from "react";

export type NoticeTone = "info" | "warn" | "error";

const DEFAULT_ICON: Record<NoticeTone, IconName> = {
  info: "dot",
  warn: "alert",
  error: "alert",
};

/** One inline message, the same everywhere a screen has something to say
 *  beside the control it is about: a failed action, a link that is down, a
 *  step in progress. A neutral surface with a tinted leading icon rather than
 *  a tinted box, so several tones sit on one screen without shouting.
 *
 *  Dismissible only when given `onDismiss`: a message tied to live state (the
 *  connection, a listing that failed) has no sensible dismiss and stays until
 *  the state changes; one about a finished action can be swiped away. */
export function Notice({
  tone = "info",
  icon,
  children,
  action,
  onDismiss,
  className,
}: {
  tone?: NoticeTone;
  /** Leading glyph. Pass `null` for none; defaults per tone. */
  icon?: IconName | ReactNode | null;
  children: ReactNode;
  /** One trailing action, e.g. Retry. */
  action?: { label: string; onClick: () => void; disabled?: boolean };
  onDismiss?: () => void;
  className?: string;
}) {
  const lead =
    icon === null ? null : icon === undefined || typeof icon === "string" ? (
      <Icon name={(icon as IconName | undefined) ?? DEFAULT_ICON[tone]} size={15} />
    ) : (
      icon
    );
  return (
    <div
      className={`notice-bar ${tone}${className ? ` ${className}` : ""}`}
      role={tone === "error" ? "alert" : "status"}
    >
      {lead && <span className="lead">{lead}</span>}
      <span className="txt">{children}</span>
      {action && (
        <button type="button" className="act" onClick={action.onClick} disabled={action.disabled}>
          {action.label}
        </button>
      )}
      {onDismiss && (
        <button type="button" className="x" onClick={onDismiss} aria-label="Dismiss">
          <Icon name="close" size={14} strokeWidth={2} />
        </button>
      )}
    </div>
  );
}
