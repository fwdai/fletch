import { useState } from "react";
import { useAppStore } from "../store";

export function SpawnDialog({ onClose }: { onClose: () => void }) {
  const spawn = useAppStore((s) => s.spawn);
  const busy = useAppStore((s) => s.busy);
  const lastError = useAppStore((s) => s.lastError);

  const [task, setTask] = useState("");

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = task.trim();
    if (!trimmed) return;
    // Name + branch are derived server-side from an auto-allocated
    // place id (e.g. `yosemite` → branch `agent/yosemite`). The user
    // only needs to describe what the agent should do.
    const rec = await spawn(trimmed, "custom");
    if (rec) onClose();
  }

  return (
    <>
      <div className="backdrop" onClick={onClose} role="presentation" />
      <div className="modal" role="dialog" aria-label="Spawn agent">
        <form onSubmit={onSubmit}>
          <h2>Spawn agent</h2>
          <label>
            <span>Task</span>
            <textarea
              autoFocus
              value={task}
              onChange={(e) => setTask(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                  e.preventDefault();
                  onSubmit(e as unknown as React.FormEvent);
                }
              }}
              placeholder="What should this agent do? Plain English instructions."
              rows={6}
            />
            <small>
              Branch and worktree name are assigned automatically.
            </small>
          </label>
          {lastError && <div className="formerr">{lastError}</div>}
          <div className="actions">
            <button type="button" onClick={onClose}>
              Cancel
            </button>
            <button
              type="submit"
              className="primary"
              disabled={busy || !task.trim()}
            >
              {busy ? "Spawning…" : "Spawn"}
            </button>
          </div>
        </form>
      </div>
    </>
  );
}
