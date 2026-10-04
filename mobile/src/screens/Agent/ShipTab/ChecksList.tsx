import type { CheckRun, PrChecks } from "@desktop/api/types/pr";
import { Icon } from "@desktop/components/Icon";
import { checkOutcome } from "./checks";

const MAX_ROWS = 20;
/** Failing first, then still running, then passed — the actionable rows lead. */
const WEIGHT = { failed: 0, pending: 1, passed: 2 } as const;

/** The PR's checks one per row, unfolded under the Checks evidence row. */
export function ChecksList({ checks }: { checks: PrChecks }) {
  if (checks.runs.length === 0) return null;
  const runs = [...checks.runs].sort((a, b) => WEIGHT[checkOutcome(a)] - WEIGHT[checkOutcome(b)]);
  const shown = runs.slice(0, MAX_ROWS);
  const hidden = runs.length - shown.length;
  return (
    <div className="checks-list">
      {shown.map((run) => (
        <CheckRow key={`${run.name}:${run.url ?? ""}`} run={run} />
      ))}
      {hidden > 0 && <div className="more">+{hidden} more</div>}
    </div>
  );
}

function CheckRow({ run }: { run: CheckRun }) {
  const outcome = checkOutcome(run);
  return (
    <div className={`check-row ${outcome}`}>
      <span className="glyph">
        {outcome === "passed" ? (
          <Icon name="check" size={13} strokeWidth={2.5} />
        ) : outcome === "failed" ? (
          <Icon name="close" size={13} strokeWidth={2.5} />
        ) : (
          <span className="working-dots">
            <i />
            <i />
            <i />
          </span>
        )}
      </span>
      <span className="nm">{run.name}</span>
      {run.required && <span className="req">required</span>}
      {run.url && (
        <a href={run.url} target="_blank" rel="noreferrer" aria-label={`Open ${run.name}`}>
          <Icon name="external" size={12} />
        </a>
      )}
    </div>
  );
}
