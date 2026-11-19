import { useState } from "react";
import type { CloudProviderStatus, CloudRunnerStatus } from "@/api";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { StatusLine } from "../StatusLine";
import { DeleteRunner } from "./DeleteRunner";
import { PairDevice } from "./PairDevice";
import { runnerActions, stateWords } from "./words";

/** A runner that exists, set up or not: its name and where it is, Start or
 *  Stop as that allows, another device's pairing code while it runs, and
 *  Delete. */
export function RunnerCard({
  provider,
  status,
  onDeleted,
}: {
  provider: CloudProviderStatus;
  status: CloudRunnerStatus;
  onDeleted: (warning: string | null) => void;
}) {
  const startCloudRunner = useAppStore((s) => s.startCloudRunner);
  const stopCloudRunner = useAppStore((s) => s.stopCloudRunner);
  const [busy, setBusy] = useState<"start" | "stop" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const words = stateWords(status, provider.label);
  const can = runnerActions(status.state);
  const name = status.record?.name || "Runner";

  const act = async (which: "start" | "stop") => {
    setBusy(which);
    setError(null);
    try {
      await (which === "start" ? startCloudRunner() : stopCloudRunner());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <>
      <StatusLine status={`${name} · ${words.short}`}>
        {(can.start || busy === "start") && (
          <Button variant="primary" disabled={!!busy} onClick={() => void act("start")}>
            {busy === "start" ? "Starting…" : "Start"}
          </Button>
        )}
        {(can.stop || busy === "stop") && (
          <Button variant="outline" disabled={!!busy} onClick={() => void act("stop")}>
            {busy === "stop" ? "Stopping…" : "Stop"}
          </Button>
        )}
      </StatusLine>
      {words.detail && <div className="set-gh-hint text-sm">{words.detail}</div>}
      {error && <div className="set-gh-err text-sm">{error}</div>}
      {can.pair && !busy && (
        <div>
          <PairDevice />
        </div>
      )}
      {can.delete && !busy && (
        <div>
          <DeleteRunner provider={provider} status={status} onDeleted={onDeleted} />
        </div>
      )}
    </>
  );
}
