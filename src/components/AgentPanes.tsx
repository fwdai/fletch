import { EMPTY_AGENTS, useAppStore } from "../store";
import { AgentTerminal } from "./AgentTerminal";

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
          its own worktree under <code>.worktrees/</code> and runs claude
          under a macOS sandbox restricted to that worktree.
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
        <p>Pick one from the sidebar to attach to its terminal.</p>
      </div>
    );
  } else {
    // Only the selected agent's terminal is mounted. xterm.js can't
    // initialize its renderer on a display:none element (throws
    // `_renderer.value.dimensions undefined`), so we tear down on
    // unselect and replay the per-agent output buffer on remount.
    // `key={selected.id}` forces a fresh mount when switching agents.
    content = <AgentTerminal key={selected.id} agent={selected} />;
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
