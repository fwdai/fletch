import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "../store";

export function ChooseRepoDialog({ onClose }: { onClose: () => void }) {
  const workspace = useAppStore((s) => s.workspace);
  const setRepo = useAppStore((s) => s.setRepo);
  const busy = useAppStore((s) => s.busy);
  const lastError = useAppStore((s) => s.lastError);

  const [path, setPath] = useState(workspace?.repo_path ?? "");

  async function pickDirectory() {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Select a git repository",
    });
    if (typeof selected === "string") setPath(selected);
  }

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!path.trim()) return;
    await setRepo(path.trim());
    if (!useAppStore.getState().lastError) onClose();
  }

  return (
    <>
      <div className="backdrop" onClick={onClose} role="presentation" />
      <div className="modal" role="dialog" aria-label="Choose repository">
        <form onSubmit={onSubmit}>
          <h2>Choose repository</h2>
          <label>
            <span>Repository path</span>
            <div className="row-input">
              <input
                value={path}
                onChange={(e) => setPath(e.target.value)}
                placeholder="/Users/you/code/your-repo"
              />
              <button type="button" onClick={pickDirectory}>
                Browse…
              </button>
            </div>
            <small>
              Each agent will get its own worktree under{" "}
              <code>.worktrees/</code> on a fresh branch, and run claude
              under a macOS sandbox (kernel-enforced — agents can only
              modify their own worktree).
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
              disabled={busy || !path.trim()}
            >
              {busy ? "Setting…" : "Use this repo"}
            </button>
          </div>
        </form>
      </div>
    </>
  );
}
