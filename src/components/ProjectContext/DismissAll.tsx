import { useState } from "react";
import { api, type ContextProposal } from "@/api";
import { Button } from "@/components/ui/Button";

/** Clears the review queue in one ruling (each dismissed as trivial). Two
 *  clicks, since nothing brings a dismissed proposal back. The first click
 *  snapshots the ids on screen and the second submits that snapshot, so a
 *  proposal that arrives in between (the queue re-renders on every change)
 *  is never ruled on unseen. */
export function DismissAll({
  projectId,
  proposals,
}: {
  projectId: string;
  proposals: ContextProposal[];
}) {
  // The ids armed by the first click; null when not confirming.
  const [armed, setArmed] = useState<string[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const dismiss = (ids: string[]) => {
    setBusy(true);
    setError(null);
    api
      .contextDismissProposals(projectId, ids)
      .catch((e) => setError(String(e)))
      .finally(() => {
        setBusy(false);
        setArmed(null);
      });
  };

  return (
    <div className="pc-inline-form text-sm">
      {armed ? (
        <>
          <Button variant="outline" danger size="sm" disabled={busy} onClick={() => dismiss(armed)}>
            Really dismiss {armed.length}?
          </Button>
          <Button variant="ghost" size="sm" disabled={busy} onClick={() => setArmed(null)}>
            Cancel
          </Button>
        </>
      ) : (
        <Button variant="outline" size="sm" onClick={() => setArmed(proposals.map((p) => p.id))}>
          Dismiss all {proposals.length}
        </Button>
      )}
      {error && <div className="pc-error text-sm">{error}</div>}
    </div>
  );
}
