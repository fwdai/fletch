import type { DictationModelProgressEvent, DictationModelStatus } from "@/api";
import { Button } from "@/components/ui/Button";
import { downloadPercent, formatBytes } from "@/util/format";

/** The line under the engine toggle: what the weights are doing and the one
 *  action that applies. Shown while the engine is on, and also while it's off
 *  but the model is still on disk — otherwise turning the engine off would
 *  strand half a gigabyte with no way to reclaim it. */
export function ModelStatusRow({
  enabled,
  status,
  progress,
  onDownload,
  onRemove,
}: {
  enabled: boolean;
  status: DictationModelStatus;
  progress: DictationModelProgressEvent | null;
  onDownload: () => void;
  onRemove: () => void;
}) {
  const size = formatBytes(status.model.size);
  const error = progress?.state === "error" ? progress.error || "Download failed" : null;
  const busy =
    status.downloading || progress?.state === "downloading" || progress?.state === "verifying";

  if (!enabled && !status.installed && !busy && !error) return null;

  let line: React.ReactNode;
  let action: React.ReactNode;
  let bar: number | null | undefined;

  if (error) {
    line = <span className="set-dict-status err">{error}</span>;
    action = (
      <Button variant="outline" size="sm" onClick={onDownload}>
        Retry
      </Button>
    );
  } else if (busy) {
    // No progress event yet (a screen that opened mid-download) means no
    // percentage to show, so the bar runs indeterminate.
    const pct = progress ? downloadPercent(progress.received, progress.total) : null;
    bar = pct;
    line = (
      <span className="set-dict-status">
        {progress?.state === "verifying"
          ? "Verifying…"
          : pct === null
            ? "Downloading…"
            : `Downloading ${pct}% · ${formatBytes(progress?.received ?? 0)} of ${size}`}
      </span>
    );
  } else if (status.installed) {
    line = <span className="set-dict-status">Installed · {size}</span>;
    action = (
      <Button variant="outline" size="sm" onClick={onRemove}>
        Remove
      </Button>
    );
  } else {
    line = <span className="set-dict-status">Not downloaded · {size}</span>;
    action = (
      <Button variant="outline" size="sm" onClick={onDownload}>
        Download
      </Button>
    );
  }

  return (
    <div className="set-dict">
      <div className="set-dict-head flex-center">
        {line}
        {action}
      </div>
      {bar !== undefined && (
        <div className={`set-dict-bar ${bar === null ? "indet" : ""}`}>
          <i style={bar === null ? undefined : { width: `${bar}%` }} />
        </div>
      )}
    </div>
  );
}
