import { useState } from "react";
import { api, type TidyCandidate } from "@/api";
import { Button } from "@/components/ui/Button";

/** "Tidy": what the log already holds that the ingester would not land today —
 *  a merged PR's line twinning the branch's own record, or a head restating a
 *  statement a later record replaced. Lists first; retracts only what the user
 *  confirms, one `retracted` event each, with the pass's reason. */
export function Tidy({ projectId, disabled }: { projectId: string; disabled: boolean }) {
  const [busy, setBusy] = useState(false);
  const [found, setFound] = useState<TidyCandidate[] | null>(null);
  const [retracted, setRetracted] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const run = (apply: boolean) => {
    setBusy(true);
    setError(null);
    api
      .contextTidy(projectId, apply)
      .then((list) => {
        if (apply) {
          setRetracted(list.length);
          setFound(null);
        } else {
          setFound(list);
          setRetracted(null);
        }
      })
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  return (
    <div className="pc-tidy">
      <div className="pc-actions">
        <Button variant="outline" size="sm" disabled={disabled || busy} onClick={() => run(false)}>
          {busy ? "Checking…" : "Tidy"}
        </Button>
        {found?.length === 0 && <span className="pc-meta text-xs">Nothing to tidy.</span>}
        {retracted !== null && (
          <span className="pc-meta text-xs">
            Retracted {retracted}; each one is in the history with its reason.
          </span>
        )}
        {error && <span className="pc-error text-sm">{error}</span>}
      </div>
      {found && found.length > 0 && (
        <>
          <ul className="pc-tidy-list text-xs">
            {found.map((c) => (
              <li key={c.retract}>
                <s>{c.statement}</s>{" "}
                <span className="pc-meta">
                  {c.reason === "twin_of" ? "twin of" : "restates the replaced"} “{c.of_statement}”
                </span>
              </li>
            ))}
          </ul>
          <div className="pc-actions">
            <Button variant="outline" size="sm" danger disabled={busy} onClick={() => run(true)}>
              Retract {found.length}
            </Button>
            <Button variant="ghost" size="sm" disabled={busy} onClick={() => setFound(null)}>
              Keep them
            </Button>
          </div>
        </>
      )}
    </div>
  );
}
