import { useAppStore } from "../store";

function basename(p: string): string {
  const parts = p.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

export function WorkspaceBar({ onChooseRepo }: { onChooseRepo: () => void }) {
  const workspace = useAppStore((s) => s.workspace);

  return (
    <header className="bar">
      <div className="left">
        <span className="logo">amux</span>
        {workspace ? (
          <span className="repo" title={workspace.repo_path}>
            {basename(workspace.repo_path)}
          </span>
        ) : (
          <span className="repo dim">No repo selected</span>
        )}
      </div>
      <div className="right">
        <button onClick={onChooseRepo}>
          {workspace ? "Switch repo…" : "Choose repo…"}
        </button>
      </div>
    </header>
  );
}
