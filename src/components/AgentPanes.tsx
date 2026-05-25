import { EMPTY_AGENTS, useAppStore } from "../store";
import { CustomAgentView } from "./CustomAgentView";
import { NativeAgentView } from "./NativeAgentView";

export function AgentPanes() {
  const workspace = useAppStore((s) => s.workspace);
  const agents = useAppStore((s) => s.workspace?.agents ?? EMPTY_AGENTS);
  const selectedId = useAppStore((s) => s.selectedAgentId);
  const lastError = useAppStore((s) => s.lastError);
  const clearError = useAppStore((s) => s.clearError);

  const selected = agents.find((a) => a.id === selectedId);

  let content;
  if (!workspace) {
    content = (
      <div className="placeholder">
        <h2>Pick a repo to get started</h2>
        <p>
          Choose a git repository in the top bar. Each agent you spawn gets
          its own worktree under <code>.worktrees/</code> and runs claude in
          a write-restricted macOS sandbox.
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
  } else if (!selected) {
    content = (
      <div className="placeholder">
        <h2>Select an agent</h2>
        <p>Pick one from the sidebar to attach.</p>
      </div>
    );
  } else if (selected.view === "native") {
    // Force re-mount when switching agents OR views — both views own
    // their own xterm / message-list and shouldn't carry state across.
    content = (
      <NativeAgentView key={`${selected.id}:native`} agent={selected} />
    );
  } else {
    content = (
      <CustomAgentView key={`${selected.id}:custom`} agent={selected} />
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
