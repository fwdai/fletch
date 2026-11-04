// NeedsYou/WorkflowReviewModal.tsx — mounts the run monitor's ReviewModal over a
// workflow run straight from the decision strip, so an approval can be decided
// without visiting the run's tab. The modal is store-agnostic by design; this
// only supplies what the run tab already holds — the run and its latest
// `gate_evidence` event — from a detail subscription of its own.

import { useEffect, useMemo } from "react";
import type { GateEvidence } from "@/api";
import { useAppStore } from "@/store";
import { ReviewModal } from "@/workflows/run/RunView/PausedBanner/ReviewModal";
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

  // The first load settled with no run — deleted, or a fetch failure the hook
  // swallows and only retries on a later event or a remount. A paused approval
  // emits nothing further, so leaving this mounted empty would make the strip's
  // Review look dead: close instead, so the next click remounts and retries.
  useEffect(() => {
    if (!loading && !run) {
      setLastError("Couldn't open the review for this run. Try again.");
      onClose();
    }
  }, [loading, run, onClose, setLastError]);

  if (!run) return null;
  return (
    <ReviewModal
      run={run}
      evidence={evidence}
      pending={loading}
      onClose={onClose}
      onError={setLastError}
    />
  );
}
