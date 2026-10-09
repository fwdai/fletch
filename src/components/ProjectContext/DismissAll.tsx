import { useState } from "react";
import { api, type ContextProposal } from "@/api";
import { Button } from "@/components/ui/Button";

/** Clears the review queue in one ruling (each dismissed as trivial). Two
 *  clicks, since nothing brings a dismissed proposal back; only the
 *  proposals on screen, not any that arrive while the person confirms. */
export function DismissAll({
  projectId,
  proposals,
}: {
  projectId: string;
  proposals: ContextProposal[];
}) {
  const count = proposals.length;
  const newest = proposals.reduce((max, p) => Math.max(max, p.created_at), 0);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const dismissAll = () => {
    setBusy(true);
    setError(null);
    api
      .contextDismissAll(projectId, newest)
      .catch((e) => setError(String(e)))
      .finally(() => {
        setBusy(false);
        setConfirming(false);
      });
  };

  return (
    <div className="pc-inline-form text-sm">
      {confirming ? (
        <>
          <Button variant="outline" danger size="sm" disabled={busy} onClick={dismissAll}>
            Really dismiss {count}?
          </Button>
          <Button variant="ghost" size="sm" disabled={busy} onClick={() => setConfirming(false)}>
            Cancel
          </Button>
        </>
      ) : (
        <Button variant="outline" size="sm" onClick={() => setConfirming(true)}>
          Dismiss all {count}
        </Button>
      )}
      {error && <div className="pc-error text-sm">{error}</div>}
    </div>
  );
}
