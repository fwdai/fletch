import { useAppStore } from "../store";
import type { AgentStatus } from "../api";

function statusColor(s: AgentStatus): string {
  switch (s) {
    case "running":
      return "var(--success)";
    case "spawning":
      return "var(--warning)";
    case "error":
      return "var(--danger)";
    default:
      return "var(--text-muted)";
  }
}

export function AgentList() {
  const agents = useAppStore((s) => s.workspace?.agents ?? []);
  const selectedId = useAppStore((s) => s.selectedAgentId);
  const selectAgent = useAppStore((s) => s.selectAgent);

  if (agents.length === 0) {
    return (
      <div className="list">
        <div className="empty">No agents yet. Click + Spawn to start one.</div>
      </div>
    );
  }

  return (
    <div className="list">
      {agents.map((agent) => (
        <button
          key={agent.id}
          className={`row ${selectedId === agent.id ? "selected" : ""}`}
          onClick={() => selectAgent(agent.id)}
        >
          <span
            className="dot"
            style={{ background: statusColor(agent.status) }}
          />
          <div className="rowtext">
            <div className="name">{agent.name}</div>
            <div className="meta">
              <span>{agent.status}</span>
              <span className="dim">·</span>
              <span className="branch">{agent.branch}</span>
            </div>
          </div>
        </button>
      ))}
    </div>
  );
}
