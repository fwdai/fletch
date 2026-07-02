import { useState } from "react";
import { useAppStore } from "@/store";
import type { AppSlice } from "@/store/types";
import { restartForUpdate } from "@/util/autoUpdate";
import { Icon, type IconName } from "./Icon";
import { Button } from "./ui/Button";

/**
 * Bottom-right update toast. Prefers the sticky "update ready → restart" prompt
 * when one is staged; otherwise surfaces transient feedback from a manual
 * "Check for Updates…" run (checking / up to date / failed). Renders nothing
 * when there's neither.
 */
export function UpdateToast() {
  const version = useAppStore((s) => s.updateReadyVersion);
  const status = useAppStore((s) => s.updateCheckStatus);

  if (version) return <UpdateReadyToast version={version} />;
  if (status) return <UpdateStatusToast status={status} />;
  return null;
}

/** Update downloaded + staged: offer "Restart now" or "Skip for now". */
function UpdateReadyToast({ version }: { version: string }) {
  const dismiss = useAppStore((s) => s.dismissUpdate);
  const [restarting, setRestarting] = useState(false);

  const onRestart = async () => {
    setRestarting(true);
    try {
      await restartForUpdate();
    } catch (err) {
      // Relaunch shouldn't fail, but if it does, don't trap the user in a
      // disabled state — let them dismiss and restart manually later.
      console.warn("Relaunch for update failed:", err);
      setRestarting(false);
    }
  };

  return (
    <div className="update-toast" role="alert">
      <Icon name="download" />
      <div className="update-toast-body">
        <div className="update-toast-text">
          <strong>Update ready</strong>
          <span>Version {version} has been downloaded.</span>
        </div>
        <div className="update-toast-actions">
          <Button variant="ghost" onClick={dismiss} disabled={restarting}>
            Skip for now
          </Button>
          <Button variant="primary" onClick={onRestart} disabled={restarting}>
            {restarting ? "Restarting…" : "Restart now"}
          </Button>
        </div>
      </div>
    </div>
  );
}

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
function UpdateStatusToast({ status }: { status: CheckStatus }) {
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
