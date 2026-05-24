import { EMPTY_AGENTS, useAppStore } from "../store";
import { AgentTerminal } from "./AgentTerminal";

export function AgentPanes() {
  const workspace = useAppStore((s) => s.workspace);
  const agents = useAppStore((s) => s.workspace?.agents ?? EMPTY_AGENTS);
  const selectedId = useAppStore((s) => s.selectedAgentId);
  const lastError = useAppStore((s) => s.lastError);
  const clearError = useAppStore((s) => s.clearError);

  let content;
  if (!workspace) {
    content = (
      <div className="placeholder">
        <h2>Pick a repo to get started</h2>
        <p>
          Choose a git repository in the top bar. Each agent you spawn will get
          its own worktree under <code>.worktrees/</code> and a fresh Tart VM
          cloned from your base image.
        </p>
      </div>
    );
  } else if (agents.length === 0) {
    content = (
      <div className="placeholder">
        <h2>No agents yet</h2>
        <p>
          Click <strong>+ Spawn</strong> in the sidebar to launch one.
        </p>
      </div>
    );
  } else if (!selectedId) {
    content = (
      <div className="placeholder">
        <h2>Select an agent</h2>
        <p>Pick one from the sidebar to attach to its terminal.</p>
      </div>
    );
  } else {
    // Render *all* agents but only show the selected one. Keeps terminal
    // state alive when the user switches away and back.
    content = (
      <>
        {agents.map((agent) => (
          <div
            key={agent.id}
            className="termhost"
            style={{ display: selectedId === agent.id ? "flex" : "none" }}
          >
            <AgentTerminal agent={agent} />
          </div>
        ))}
      </>
    );
  }

  return (
    <div className="panes">
      {content}
      {lastError && (
        <div className="error" role="alert">
          {lastError}
          <button className="close" onClick={clearError}>
            ×
          </button>
        </div>
      )}
    </div>
  );
}
