import type { DictationModel, DictationModelProgressEvent, DictationModelStatus } from "@/api";
import { Button } from "@/components/ui/Button";
import { downloadPercent, formatBytes } from "@/util/format";

/** What one model's weights are doing and the single action that applies, for
 *  the line under its label in the chooser. Both the in-flight flag and the
 *  progress event are process-wide, so each is checked against this model:
 *  changing the selection doesn't cancel a download, and the bar belongs on
 *  whichever row is actually being fetched. */
export function ModelStatusRow({
  model,
  status,
  progress,
  onDownload,
  onRemove,
}: {
  model: DictationModel;
  status: DictationModelStatus;
  progress: DictationModelProgressEvent | null;
  onDownload: () => void;
  onRemove: () => void;
}) {
  const size = formatBytes(model.size);
  const mine = progress?.model_id === model.id ? progress : null;
  const error = mine?.state === "error" ? mine.error || "Download failed" : null;
  const busy =
    status.downloading_id === model.id ||
    mine?.state === "downloading" ||
    mine?.state === "verifying";

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
    const pct = mine ? downloadPercent(mine.received, mine.total) : null;
    bar = pct;
    line = (
      <span className="set-dict-status">
        {mine?.state === "verifying"
          ? "Verifying…"
          : pct === null
            ? "Downloading…"
            : `Downloading ${pct}% · ${formatBytes(mine?.received ?? 0)} of ${size}`}
      </span>
    );
  } else if (model.installed) {
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
