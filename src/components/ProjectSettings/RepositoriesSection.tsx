import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";

/** The repositories attached to this project. One repo is the common case;
 *  attaching more turns it into a multi-repo project grouped under a single
 *  sidebar entry. The first (primary) repo is where new agents start. */
export function RepositoriesSection({ projectId }: { projectId: string }) {
  const projects = useAppStore((s) => s.workspace?.projects);
  const attachRepoToProject = useAppStore((s) => s.attachRepoToProject);
  const detachRepoFromProject = useAppStore((s) => s.detachRepoFromProject);
  const [adding, setAdding] = useState(false);
  const [busyPath, setBusyPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const repos = (projects ?? []).filter((p) => p.project_id === projectId).map((p) => p.path);

  async function onAttach() {
    if (adding) return;
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Select a git repository to attach",
    });
    if (typeof picked !== "string") return;
    setAdding(true);
    setError(null);
    try {
      await attachRepoToProject(projectId, picked);
    } catch (e) {
      setError(String(e));
    } finally {
      setAdding(false);
    }
  }

  async function onDetach(path: string) {
    if (busyPath) return;
    setBusyPath(path);
    setError(null);
    try {
      await detachRepoFromProject(projectId, path);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyPath(null);
    }
  }

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Repositories</h2>
        <p className="ps-section-lead text-sm">
          The repositories that make up this project. Attach more to group several repos — a
          frontend, a backend — under one project.
        </p>
      </header>

      <div className="ps-repo-list">
        {repos.map((path, i) => (
          <div key={path} className="ps-repo-row flex-center">
            <Icon name="folder" size={14} />
            <span className="ps-repo-path mono text-sm truncate" title={path}>
              {path}
            </span>
            {i === 0 && repos.length > 1 && (
              <span className="ps-repo-primary text-xs">primary</span>
            )}
            {repos.length > 1 && (
              <button
                className="ps-repo-x iflex-center tip"
                data-tip="Detach from project"
                aria-label={`Detach ${path}`}
                disabled={busyPath !== null}
                onClick={() => void onDetach(path)}
              >
                <Icon name="close" size={12} />
              </button>
            )}
          </div>
        ))}
      </div>

      <button
        type="button"
        className="ps-btn ps-repo-add"
        disabled={adding}
        onClick={() => void onAttach()}
      >
        {adding ? "Attaching…" : "Attach repository…"}
      </button>
      <p className="ps-hint text-xs">
        New agents currently start in the primary repository. Detaching never touches the folder on
        disk.
      </p>

      {error && <div className="ps-error text-sm">{error}</div>}
    </section>
  );
}
