import { open } from "@tauri-apps/plugin-shell";
import type { CheckRun, PrChecks } from "../../../../api";
import { Icon } from "../../../Icon";

/** Visual class for one check row: ok / fail / skip dot, or a spinner while
 *  the run is queued / in progress. */
function checkTone(run: CheckRun): "ok" | "fail" | "skip" | "run" {
  if (run.status !== "completed") return "run";
  switch (run.conclusion) {
    case "success":
      return "ok";
    case "neutral":
    case "skipped":
    case "stale":
    case null:
      return "skip";
    default:
      return "fail"; // failure, timed_out, cancelled, action_required, …
  }
}

export function ChecksSection({ checks, prUrl }: { checks: PrChecks; prUrl: string }) {
  if (checks.total === 0) return null;
  // Failing first, then running, then the rest — the actionable rows lead.
  const weight = (r: CheckRun) => (checkTone(r) === "fail" ? 0 : checkTone(r) === "run" ? 1 : 2);
  const runs = [...checks.runs].sort((a, b) => weight(a) - weight(b));
  const shown = runs.slice(0, 6);
  const hidden = runs.length - shown.length;
  const summary =
    checks.rollup === "failing"
      ? `${checks.failed} failing`
      : checks.rollup === "pending"
        ? `${checks.total - checks.pending} of ${checks.total} done`
        : "all passing";
  return (
    <div className="pr-checks">
      <div className="pr-checks-h text-2xs">
        <span>Checks</span>
        <span className={`pr-checks-sum ${checks.rollup}`}>{summary}</span>
      </div>
      {shown.map((r) => {
        const tone = checkTone(r);
        return (
          <button
            type="button"
            key={r.name}
            className="pr-check flex-center"
            onClick={() => void open(r.url ?? `${prUrl}/checks`)}
          >
            {tone === "run" ? (
              <span className="git-spin sm" />
            ) : (
              <span className={`pc-dot ${tone}`} />
            )}
            <span className="pc-name">{r.name}</span>
            <Icon name="external" size={10} />
          </button>
        );
      })}
      {hidden > 0 && (
        <button
          type="button"
          className="pr-checks-more"
          onClick={() => void open(`${prUrl}/checks`)}
        >
          +{hidden} more on GitHub
        </button>
      )}
    </div>
  );
}
