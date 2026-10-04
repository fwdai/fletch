import type { UpdateProgress } from "@/util/autoUpdate";
import { downloadPercent, formatBytes } from "@/util/format";
import { Icon } from "../Icon";
import { ProgressBar } from "../ui/ProgressBar";

/** A found update being fetched and staged — the slow part of a manual check,
 *  so it gets a byte count and a bar rather than a "checking" that never ends. */
export function UpdateDownloadToast({ progress }: { progress: UpdateProgress }) {
  const { version, phase, received, total } = progress;
  const installing = phase === "installing";
  const pct = installing ? null : downloadPercent(received, total);

  let detail = `Version ${version}`;
  if (installing) detail += " · almost done";
  else if (pct !== null && total)
    detail += ` · ${pct}% · ${formatBytes(received)} of ${formatBytes(total)}`;
  else if (received > 0) detail += ` · ${formatBytes(received)}`;

  return (
    <div className="update-toast downloading" role="status">
      <Icon name="download" />
      <div className="update-toast-body">
        <div className="update-toast-text">
          <strong>{installing ? "Installing update…" : "Downloading update…"}</strong>
          <span>{detail}</span>
        </div>
        <ProgressBar percent={pct} />
      </div>
    </div>
  );
}
