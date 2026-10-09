import { useState } from "react";
import { api } from "@/api";
import { Button } from "@/components/ui/Button";

/** Clears the review queue in one ruling (each dismissed as trivial). Two
 *  clicks, since nothing brings a dismissed proposal back. */
export function DismissAll({ projectId, count }: { projectId: string; count: number }) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const dismissAll = () => {
    setBusy(true);
    setError(null);
    api
      .contextDismissAll(projectId)
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
