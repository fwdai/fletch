import { useState } from "react";
import type { CloudProviderStatus } from "@/api";
import { useAppStore } from "@/store";
import { createRunner } from "./actions";
import { CreateForm } from "./CreateForm";
import { Creating } from "./Creating";
import { RunnerCard } from "./RunnerCard";

/** This install's runner, under a connected provider: `fletch-host` on a
 *  machine there that stays on, supervising agents with this Mac closed and
 *  serving every paired device through the relay. One per install, so it
 *  shows under the provider its machine is at, and the form that creates one
 *  under each connected provider while there is none.
 *
 *  What it shows is the store's (`cloudRunner`, read by the pane through
 *  `useRunnerStatus`, and `cloudRunnerCreate` while this window creates one);
 *  what a create's failure or a forced delete's warning said is kept here, so
 *  it stays on screen as the view under it changes. */
export function Runner({
  provider,
  readError,
}: {
  provider: CloudProviderStatus;
  /** Why the runner's status could not be read, when it never was. */
  readError: string | null;
}) {
  const status = useAppStore((s) => s.cloudRunner);
  const creating = useAppStore((s) => s.cloudRunnerCreate);
  const [error, setError] = useState<string | null>(null);
  const [warning, setWarning] = useState<string | null>(null);

  if (creating && creating.provider !== provider.id) return null;
  if (!creating && status?.provider && status.provider !== provider.id) return null;
  if (!creating && !status && !readError) return null;

  // The engine has undone a failed create, so the form is back with why.
  const create = async (claudeToken: string, name: string) => {
    setError(null);
    setWarning(null);
    try {
      await createRunner(provider.id, claudeToken, name);
    } catch (e) {
      setError(String(e));
    }
  };

  const view = () => {
    if (creating) return <Creating provider={provider} steps={creating.steps} />;
    if (!status) {
      return <div className="set-gh-err text-sm">Couldn't read the runner: {readError}</div>;
    }
    if (status.state === "none") {
      return <CreateForm provider={provider} onCreate={(t, n) => void create(t, n)} />;
    }
    return (
      <RunnerCard
        provider={provider}
        status={status}
        onDeleted={(w) => {
          setError(null);
          setWarning(w);
        }}
      />
    );
  };

  return (
    <div className="set-gh set-runner">
      {view()}
      {error && <div className="set-gh-err text-sm">{error}</div>}
      {warning && <div className="set-inline-warn">{warning}</div>}
    </div>
  );
}
