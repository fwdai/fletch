import { useState } from "react";
import { useAppStore } from "../store";
import { restartForUpdate } from "../util/autoUpdate";
import { Icon } from "./Icon";

/**
 * Toast shown when an update has been downloaded and staged. Offers "Restart
 * now" (relaunch into the new version) or "Skip for now" (dismiss; the update
 * applies on the next launch regardless). Renders nothing when no update is
 * pending.
 */
export function UpdateToast() {
  const version = useAppStore((s) => s.updateReadyVersion);
  const dismiss = useAppStore((s) => s.dismissUpdate);
  const [restarting, setRestarting] = useState(false);

  if (!version) return null;

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
          <button className="btn-t ghost" onClick={dismiss} disabled={restarting}>
            Skip for now
          </button>
          <button className="btn-t primary" onClick={onRestart} disabled={restarting}>
            {restarting ? "Restarting…" : "Restart now"}
          </button>
        </div>
      </div>
    </div>
  );
}
