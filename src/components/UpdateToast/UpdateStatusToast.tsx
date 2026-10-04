import type { AppSlice } from "@/store/types";
import { Icon, type IconName } from "../Icon";

// Derived from the store so a new variant on `updateCheckStatus` can't drift.
type CheckStatus = NonNullable<AppSlice["updateCheckStatus"]>;

const STATUS_COPY: Record<CheckStatus, { icon: IconName; title: string; detail: string }> = {
  checking: {
    icon: "refresh",
    title: "Checking for updates…",
    detail: "Contacting the update server.",
  },
  uptodate: {
    icon: "check",
    title: "You're up to date",
    detail: "Fletch is running the latest version.",
  },
  error: {
    icon: "close",
    title: "Update check failed",
    detail: "Couldn't reach the update server.",
  },
};

/** Transient feedback for a manual check — auto-dismissed by the store. */
export function UpdateStatusToast({ status }: { status: CheckStatus }) {
  const { icon, title, detail } = STATUS_COPY[status];
  return (
    <div className="update-toast" role="status">
      <Icon name={icon} />
      <div className="update-toast-body">
        <div className="update-toast-text">
          <strong>{title}</strong>
          <span>{detail}</span>
        </div>
      </div>
    </div>
  );
}
