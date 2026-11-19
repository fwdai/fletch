import { useState } from "react";
import type { CloudProviderStatus } from "@/api";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";

type Step = "idle" | "confirm" | "refused";

/** Delete, behind a confirmation that says what goes with the runner, as the
 *  Providers pane's account rows confirm. The engine refuses a runner it
 *  cannot reach (or one that could not destroy every machine it made); its
 *  words are shown, and the forced delete is offered with what it may leave
 *  behind. */
export function DeleteRunner({
  provider,
  onDeleted,
}: {
  provider: CloudProviderStatus;
  /** The runner is gone; `warning` is what a forced delete may have left. */
  onDeleted: (warning: string | null) => void;
}) {
  const deleteCloudRunner = useAppStore((s) => s.deleteCloudRunner);
  const [step, setStep] = useState<Step>("idle");
  const [refusal, setRefusal] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const label = provider.label;

  const run = async (force: boolean) => {
    setBusy(true);
    try {
      const deleted = await deleteCloudRunner(force);
      setStep("idle");
      onDeleted(deleted.warning);
    } catch (e) {
      setRefusal(String(e));
      setStep("refused");
    } finally {
      setBusy(false);
    }
  };

  if (step === "idle") {
    return (
      <Button variant="outline" size="sm" danger onClick={() => setStep("confirm")}>
        Delete runner
      </Button>
    );
  }

  const cancel = (
    <Button variant="ghost" size="sm" disabled={busy} onClick={() => setStep("idle")}>
      Cancel
    </Button>
  );
  const waiting = busy && (
    <div className="set-gh-hint text-sm">
      A stopped runner is started first, so it can delete the machines it made. This can take a few
      minutes.
    </div>
  );

  if (step === "confirm") {
    return (
      <div className="set-runner-confirm">
        <div className="set-gh-lede text-sm">
          Delete the runner? The machines it made at {label}, its workspaces' and its spares, are
          deleted first, then its own machine and disk, and with them its history, logins and
          pairings.
        </div>
        <div className="set-runner-actions flex-center">
          <Button
            variant="outline"
            size="sm"
            danger
            disabled={busy}
            onClick={() => void run(false)}
          >
            {busy ? "Deleting…" : "Delete"}
          </Button>
          {cancel}
        </div>
        {waiting}
      </div>
    );
  }

  return (
    <div className="set-runner-confirm">
      <div className="set-gh-err text-sm">{refusal}</div>
      <div className="set-gh-lede text-sm">
        Delete it anyway? Its own machine and disk are deleted, but the machines it made, its
        workspaces' and its spares, may remain at {label}, for you to delete there.
      </div>
      <div className="set-runner-actions flex-center">
        <Button variant="outline" size="sm" danger disabled={busy} onClick={() => void run(true)}>
          {busy ? "Deleting…" : "Delete anyway"}
        </Button>
        {cancel}
      </div>
      {waiting}
    </div>
  );
}
