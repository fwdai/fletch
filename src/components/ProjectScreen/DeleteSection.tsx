import { useEffect, useState } from "react";
import { api } from "@/api";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";

export function DeleteSection({
  projectId,
  projectName,
}: {
  projectId: string;
  projectName: string;
}) {
  const agents = useAppStore((state) => state.workspace?.agents ?? []);
  const deleteProject = useAppStore((state) => state.deleteProject);
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
  }, [projectId]);

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

  return (
    <section className="ps-section ps-delete-section">
      {confirming ? (
        <div className="ps-delete-confirm" role="group" aria-label="Confirm project deletion">
          <div className="ps-delete-confirm-copy">
            <h3 className="ps-delete-confirm-title text-base">Delete {projectName}?</h3>
            <div className="ps-delete-confirm-text text-sm">
              This permanently deletes the project and all of its agents, workspaces, and history.
              This action can&rsquo;t be undone.
            </div>
            {hasRunningAgent && (
              <div className="ps-delete-note text-xs">
                Stop running agents before deleting this project.
              </div>
            )}
            {error && <div className="ps-delete-error text-xs">{error}</div>}
          </div>
          <div className="ps-delete-actions">
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
              disabled={checkingAgents || hasRunningAgent || deleting}
              onClick={() => void remove()}
            >
              {deleting ? "Deleting…" : "Confirm delete"}
            </Button>
          </div>
        </div>
      ) : (
        <>
          <header className="ps-section-h ps-delete-copy">
            <h2 className="ps-section-t text-lg">Delete project</h2>
            <p className="ps-section-lead text-sm">
              Permanently delete {projectName} and all of its agents, workspaces, and history.
            </p>
            {hasRunningAgent && (
              <div className="ps-delete-note text-xs">
                Stop running agents before deleting this project.
              </div>
            )}
            {error && <div className="ps-delete-error text-xs">{error}</div>}
          </header>
          <Button
            variant="outline"
            danger
            disabled={checkingAgents || hasRunningAgent}
            onClick={() => {
              setConfirming(true);
              setError(null);
            }}
            tip={
              checkingAgents
                ? "Checking agent status"
                : hasRunningAgent
                  ? "Stop running agents first"
                  : undefined
            }
          >
            Delete project
          </Button>
        </>
      )}
    </section>
  );
}
