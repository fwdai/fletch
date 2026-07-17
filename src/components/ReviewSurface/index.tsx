// ReviewSurface — the shared surface a human uses to decide a merge/handoff at
// a workflow approval gate (spec §9). It shows the evidence (verification, the
// ferried diff, budget spend, the step's verdict) and offers Approve / Request
// changes. It is intentionally decoupled from any store: everything arrives as
// props (evidence + a diff source + callbacks), so the next PR can mount the same
// component from a fleet review queue without change.

import type { GateEvidence } from "../../api";
import { Icon } from "../Icon";
import { BudgetSummary } from "./BudgetSummary";
import { Checks } from "./Checks";
import { DiffPanel } from "./DiffPanel";
import { ReviewActions } from "./ReviewActions";

export interface ReviewSurfaceProps {
  /** The assembled gate evidence, or `null` when none was journaled. */
  evidence: GateEvidence | null;
  /** Evidence may still arrive (events are loading). Actions stay disabled —
   *  approving before the evidence renders defeats the surface's purpose. */
  pending?: boolean;
  /** Fetch a file's unified diff (the whole diff when `path` is `null`). */
  getDiff: (path: string | null) => Promise<string>;
  /** Promote the step and advance the run. */
  onApprove: () => Promise<void>;
  /** Send the reviewer's note back to the step for another attempt. */
  onReject: (note: string) => Promise<void>;
  /** Surface an action failure (the parent's inline error channel). */
  onError?: (message: string) => void;
}

export function ReviewSurface({
  evidence,
  pending = false,
  getDiff,
  onApprove,
  onReject,
  onError,
}: ReviewSurfaceProps) {
  const waiting = pending && !evidence;
  return (
    <div className="review-surface">
      <div className="rv-body">
        {evidence ? (
          <Evidence evidence={evidence} getDiff={getDiff} />
        ) : waiting ? (
          <Preparing />
        ) : (
          <NoEvidence />
        )}
      </div>
      <div className="rv-foot">
        <ReviewActions
          disabled={waiting}
          onApprove={onApprove}
          onReject={onReject}
          onError={onError}
        />
      </div>
    </div>
  );
}

function Evidence({
  evidence,
  getDiff,
}: {
  evidence: GateEvidence;
  getDiff: (path: string | null) => Promise<string>;
}) {
  const { diff, verdict } = evidence;
  return (
    <>
      <div className="rv-summary">
        <div className="rv-tile">
          <span className="rv-tile-label">
            <Icon name="diff" size={12} /> Changes
          </span>
          <span className="rv-tile-value">
            <span className="add">+{diff.additions}</span>{" "}
            <span className="rem">−{diff.deletions}</span>
            <span className="rv-tile-sub">
              {" · "}
              {diff.files.length} {diff.files.length === 1 ? "file" : "files"}
            </span>
          </span>
        </div>
        <div className="rv-tile rv-tile-budget">
          <span className="rv-tile-label">
            <Icon name="zap" size={12} /> Budget
          </span>
          <BudgetSummary budget={evidence.budget} />
        </div>
      </div>

      {verdict && (
        <section className="rv-sect">
          <div className="rv-sect-head">
            <Icon name="notebookPen" size={13} /> Verdict
            <span className={`rv-verdict-badge v-${verdict.result}`}>{verdict.result}</span>
          </div>
          {verdict.summary.trim() ? (
            <div className="rv-verdict-summary">{verdict.summary}</div>
          ) : (
            <div className="rv-checks-note">The step left no summary.</div>
          )}
          {verdict.detail?.trim() && <pre className="rv-verdict-detail">{verdict.detail}</pre>}
        </section>
      )}

      <section className="rv-sect">
        <div className="rv-sect-head">
          <Icon name="flask" size={13} /> Checks
        </div>
        <Checks verification={evidence.verification} />
      </section>

      <section className="rv-sect rv-sect-diff">
        <div className="rv-sect-head">
          <Icon name="code" size={13} /> Diff
        </div>
        <DiffPanel files={diff.files} getDiff={getDiff} />
      </section>
    </>
  );
}

function Preparing() {
  return (
    <div className="empty-msg" style={{ margin: "auto" }}>
      <div className="et">Preparing the review…</div>
      <div>Gathering verification, the diff, and budget for this step.</div>
    </div>
  );
}

/** The pause pre-dates evidence collection (or the event was lost). Actions stay
 *  live — an evidence-less legacy pause must never be unapprovable — but the
 *  absence is stated plainly instead of an eternal "preparing" spinner. */
function NoEvidence() {
  return (
    <div className="empty-msg" style={{ margin: "auto" }}>
      <div className="et">No evidence was recorded for this pause</div>
      <div>Review the step's chat and the run timeline before deciding.</div>
    </div>
  );
}
