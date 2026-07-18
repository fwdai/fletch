import { useCallback } from "react";
import { api, type GateEvidence, type WfRun } from "../../../../api";
import { Icon } from "../../../../components/Icon";
import { ReviewSurface } from "../../../../components/ReviewSurface";
import { IconButton, Scrim } from "../../../../components/ui";

/** The approval review, framed as a modal over the run. Wires the shared
 *  `ReviewSurface` to this run's diff source and approve/reject commands; the
 *  surface itself is store-agnostic so a fleet queue can mount it elsewhere. */
export function ReviewModal({
  run,
  evidence,
  pending,
  onClose,
  onError,
}: {
  run: WfRun;
  evidence: GateEvidence | null;
  pending: boolean;
  onClose: () => void;
  onError: (message: string) => void;
}) {
  const getDiff = useCallback(
    (path: string | null): Promise<string> => {
      if (!evidence) return Promise.resolve("");
      return api.wfRunDiff(run.id, evidence.base_sha, evidence.head_sha, path ?? undefined);
    },
    [run.id, evidence],
  );

  // Close on success; the run-state effect above also catches external flips.
  const decide = useCallback(
    async (action: Promise<void>) => {
      await action;
      onClose();
    },
    [onClose],
  );

  return (
    <>
      <Scrim onClose={onClose} zIndex={399} />
      <div className="rv-modal">
        <div className="rv-modal-card">
          <div className="rv-modal-head">
            <span className="rv-modal-title">
              <Icon name="combine" size={14} style={{ color: "var(--accent)" }} /> Review
              <span className="rv-modal-step">{run.task || run.name}</span>
            </span>
            <IconButton className="rv-modal-close" tip="Close" onClick={onClose}>
              <Icon name="close" size={14} />
            </IconButton>
          </div>
          <ReviewSurface
            evidence={evidence}
            pending={pending}
            getDiff={getDiff}
            onApprove={() => decide(api.wfApprove(run.id))}
            onReject={(note) => decide(api.wfReject(run.id, note))}
            onError={onError}
          />
        </div>
      </div>
    </>
  );
}
