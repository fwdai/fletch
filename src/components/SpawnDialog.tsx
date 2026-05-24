import { useState } from "react";
import { useAppStore } from "../store";

export function SpawnDialog({ onClose }: { onClose: () => void }) {
  const spawn = useAppStore((s) => s.spawn);
  const busy = useAppStore((s) => s.busy);
  const lastError = useAppStore((s) => s.lastError);

  const [name, setName] = useState("");
  const [branch, setBranch] = useState("");
  const [task, setTask] = useState("");

  function suggestBranch() {
    if (!branch && name) {
      setBranch(
        "agent/" +
          name
            .toLowerCase()
            .replace(/[^a-z0-9-]+/g, "-")
            .slice(0, 32),
      );
    }
  }

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!name.trim() || !branch.trim() || !task.trim()) return;
    const rec = await spawn(name.trim(), branch.trim(), task.trim());
    if (rec) onClose();
  }

  return (
    <>
      <div className="backdrop" onClick={onClose} role="presentation" />
      <div className="modal" role="dialog" aria-label="Spawn agent">
        <form onSubmit={onSubmit}>
          <h2>Spawn agent</h2>
          <label>
            <span>Name</span>
            <input
              autoFocus
              value={name}
              onChange={(e) => setName(e.target.value)}
              onBlur={suggestBranch}
              placeholder="refactor-auth"
            />
          </label>
          <label>
            <span>Branch</span>
            <input
              value={branch}
              onChange={(e) => setBranch(e.target.value)}
              placeholder="agent/refactor-auth"
            />
          </label>
          <label>
            <span>Task</span>
            <textarea
              value={task}
              onChange={(e) => setTask(e.target.value)}
              placeholder="What should this agent do? Plain English instructions."
              rows={5}
            />
          </label>
          {lastError && <div className="formerr">{lastError}</div>}
          <div className="actions">
            <button type="button" onClick={onClose}>
              Cancel
            </button>
            <button type="submit" className="primary" disabled={busy}>
              {busy ? "Spawning…" : "Spawn"}
            </button>
          </div>
        </form>
      </div>
    </>
  );
}
