import { useState } from "react";
import { useAppStore } from "@/store";
import { restartForUpdate } from "@/util/autoUpdate";
import { Icon } from "../Icon";
import { Button } from "../ui/Button";

/** Update downloaded + staged: offer "Restart now" or "Skip for now". */
export function UpdateReadyToast({ version, notes }: { version: string; notes: string | null }) {
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
        {notes && <p className="update-toast-notes">{notes}</p>}
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
