import { useAppStore } from "@/store";
import { UpdateDownloadToast } from "./UpdateDownloadToast";
import { UpdateReadyToast } from "./UpdateReadyToast";
import { UpdateStatusToast } from "./UpdateStatusToast";

/**
 * Bottom-right update toast. Prefers the sticky "update ready → restart" prompt
 * when one is staged; then the download progress of an update a manual
 * "Check for Updates…" found; otherwise that run's transient feedback
 * (checking / up to date / failed). Renders nothing when there's none of these.
 */
export function UpdateToast() {
  const version = useAppStore((s) => s.updateReadyVersion);
  const notes = useAppStore((s) => s.updateReadyNotes);
  const download = useAppStore((s) => s.updateDownload);
  const status = useAppStore((s) => s.updateCheckStatus);

  if (version) return <UpdateReadyToast version={version} notes={notes} />;
  if (download) return <UpdateDownloadToast progress={download} />;
  if (status) return <UpdateStatusToast status={status} />;
  return null;
}
