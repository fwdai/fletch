import { useEffect, useState } from "react";
import { api } from "@/api";
import { SetRow } from "@/components/SettingsScreen/primitives";
import { Button } from "@/components/ui/Button";
import { EMPTY_AGENTS, useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";

/** Delete the project, behind an inline confirm in the same row. */
export function DeleteProjectRow({
  projectId,
  projectName,
}: {
  projectId: string;
  projectName: string;
}) {
  const agents = useAppStore((state) => state.workspace?.agents ?? EMPTY_AGENTS);
  const deleteProject = useAppStore((state) => state.deleteProject);
  // A host from before the project-settings ops answers neither the delete nor
  // the running-agents probe, so the gate speaks for both.
  const gate = useGate("projectAdmin");
  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hasBackendRunningAgent, setHasBackendRunningAgent] = useState<boolean | null>(null);
  const hasVisibleRunningAgent = agents.some(
    (agent) =>
      agent.project_id === projectId && (agent.status === "running" || agent.status === "spawning"),
  );
  const checkingAgents = hasBackendRunningAgent === null && !hasVisibleRunningAgent;
  const hasRunningAgent = hasVisibleRunningAgent || hasBackendRunningAgent === true;

  useEffect(() => {
    // A gated host would answer the probe `unknown op` every second, and its
    // fail-closed branch would then blame running agents for a button that is
    // dead for another reason entirely.
    if (gate) return;
    let cancelled = false;
    const refresh = async () => {
      try {
        const running = await api.projectHasRunningAgents(projectId);
        if (!cancelled) setHasBackendRunningAgent(running);
      } catch {
        // Fail closed. The delete command repeats the same guard, but keeping
        // the button disabled avoids presenting an action we cannot validate.
        if (!cancelled) setHasBackendRunningAgent(true);
      }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 1000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [projectId, gate]);

  const blocked = gate !== null || checkingAgents || hasRunningAgent;
  const blockedTip =
    gate ??
    (checkingAgents
      ? "Checking agent status"
      : hasRunningAgent
        ? "Stop running agents first"
        : undefined);

  const remove = async () => {
    setDeleting(true);
    setError(null);
    try {
      await deleteProject(projectId);
    } catch (err) {
      setError(String(err));
      setDeleting(false);
    }
  };

  const note =
    error ?? gate ?? (hasRunningAgent ? "Stop running agents before deleting this project." : null);

  return (
    <SetRow
      title={confirming ? `Delete ${projectName}?` : "Delete project"}
      sub={
        note ??
        (confirming
          ? "Its agents, workspaces and history go with it. This can't be undone."
          : `Permanently deletes ${projectName} and all of its agents, workspaces and history.`)
      }
    >
      {confirming ? (
        <>
          <Button
            variant="ghost"
            disabled={deleting}
            onClick={() => {
              setConfirming(false);
              setError(null);
            }}
          >
            Cancel
          </Button>
          <Button
            variant="outline"
            danger
            disabled={blocked || deleting}
            tip={blockedTip}
            onClick={() => void remove()}
          >
            {deleting ? "Deleting…" : "Confirm delete"}
          </Button>
        </>
      ) : (
        <Button
          variant="outline"
          danger
          disabled={blocked}
          tip={blockedTip}
          onClick={() => {
            setConfirming(true);
            setError(null);
          }}
        >
          Delete project
        </Button>
      )}
    </SetRow>
  );
}
