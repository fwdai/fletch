import { useState } from "react";
import type { RestoreReport, RewindScope } from "@/api";
import { MenuButton } from "@/components/ui/MenuButton";
import { agentRecord } from "@/helpers";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { ConfirmRestore } from "./ConfirmRestore";
import type { CodeScope } from "./confirm";
import { RewindOptions } from "./RewindOptions";

/** The rewind action on a user message: go back to just before it, in place —
 *  the conversation, the code, or both (the store's rewindAgent). Whatever
 *  restores the code is confirmed first. `prompt` is the message as the user
 *  wrote it, which a conversation rewind puts back in the composer.
 *
 *  Not offered on a remote environment (`rewind` gate) nor for a workflow
 *  step, whose conversation belongs to its run. */
export function RewindMenu({
  agentId,
  turnId,
  prompt,
}: {
  agentId: string;
  turnId: string;
  prompt: string;
}) {
  const rewindGate = useGate("rewind");
  const agent = useAppStore((s) => agentRecord(s, agentId));
  const rewindAgent = useAppStore((s) => s.rewindAgent);
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState<{
    scope: CodeScope;
    report: RestoreReport;
  } | null>(null);

  if (rewindGate || !agent || agent.owner_run_id) return null;

  const rewind = async (scope: RewindScope) => {
    setBusy(true);
    try {
      await rewindAgent(agentId, turnId, scope, prompt);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <MenuButton icon="rewind" tip="Rewind to here" compact disabled={busy}>
        {(close) => (
          <RewindOptions
            agent={agent}
            turnId={turnId}
            onPick={(scope, report) => {
              close();
              if (scope === "conversation") void rewind(scope);
              else if (report) setConfirming({ scope, report });
            }}
          />
        )}
      </MenuButton>
      {confirming && (
        <ConfirmRestore
          scope={confirming.scope}
          report={confirming.report}
          onCancel={() => setConfirming(null)}
          onConfirm={() => {
            setConfirming(null);
            void rewind(confirming.scope);
          }}
        />
      )}
    </>
  );
}
