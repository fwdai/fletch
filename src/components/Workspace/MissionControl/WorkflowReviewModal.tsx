// MissionControl/WorkflowReviewModal.tsx — mounts the shared ReviewSurface over a
// workflow run straight from the queue, so an approval can be decided without
// visiting the run's tab. Same wiring as RunView/PausedBanner's ReviewModal
// (evidence = the latest `gate_evidence` event; approve/reject via wfApprove /
// wfReject; diff via wfRunDiff) — the surface is store-agnostic by design, so
// this is a thin second mount, not a fork.

import { useCallback, useEffect, useMemo } from "react";
import { api, type GateEvidence } from "@/api";
import { Icon } from "@/components/Icon";
import { ReviewSurface } from "@/components/ReviewSurface";
import { IconButton, Scrim } from "@/components/ui";
import { useAppStore } from "@/store";
import { useRunDetail } from "@/workflows/run/RunView/useRunDetail";

export function WorkflowReviewModal({ runId, onClose }: { runId: string; onClose: () => void }) {
  const { detail, events, loading } = useRunDetail(runId);
  const run = detail?.run ?? null;
  const setLastError = useAppStore((s) => s.setLastError);

  const evidence = useMemo<GateEvidence | null>(() => {
    if (run?.status !== "paused" || run.paused_reason !== "approval") return null;
    for (let i = events.length - 1; i >= 0; i--) {
      if (events[i].type === "gate_evidence") return events[i].payload as GateEvidence;
    }
    return null;
  }, [events, run?.status, run?.paused_reason]);

  // If the run flips out of paused(approval) underneath the modal (approved
  // elsewhere, reject exhausted the budget, deleted), close it — an open surface
  // over a run that no longer awaits a decision is stale.
  useEffect(() => {
    if (run && !(run.status === "paused" && run.paused_reason === "approval")) onClose();
  }, [run, onClose]);

  const getDiff = useCallback(
    (path: string | null): Promise<string> => {
      if (!evidence) return Promise.resolve("");
      return api.wfRunDiff(runId, evidence.base_sha, evidence.head_sha, path ?? undefined);
    },
    [runId, evidence],
  );

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
              <span className="rv-modal-step">{run?.task || run?.name || "…"}</span>
            </span>
            <IconButton className="rv-modal-close" tip="Close" onClick={onClose}>
              <Icon name="close" size={14} />
            </IconButton>
          </div>
          <ReviewSurface
            evidence={evidence}
            pending={loading}
            getDiff={getDiff}
            onApprove={() => decide(api.wfApprove(runId))}
            onReject={(note) => decide(api.wfReject(runId, note))}
            onError={setLastError}
          />
        </div>
      </div>
    </>
  );
}
