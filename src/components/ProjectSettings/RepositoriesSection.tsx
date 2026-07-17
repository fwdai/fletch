import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import type { ProjectRef } from "@/api";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { basename } from "@/util/format";

/** Where the project lives on disk. Each repo row carries its own path,
 *  relocate action, and an optional label ("Frontend", "Gateway") that names
 *  it inside the project. One repo is the common case; attaching more turns
 *  the project multi-repo, grouped under a single sidebar entry. The project
 *  itself has no location — repos can live anywhere. */
export function RepositoriesSection({ projectId }: { projectId: string }) {
  const projects = useAppStore((s) => s.workspace?.projects);
  const attachRepoToProject = useAppStore((s) => s.attachRepoToProject);
  const detachRepoFromProject = useAppStore((s) => s.detachRepoFromProject);
  const relocateProject = useAppStore((s) => s.relocateProject);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const repos = (projects ?? []).filter((p) => p.project_id === projectId);
  const multi = repos.length > 1;

  async function run(fn: () => Promise<void>) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function onAttach() {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Select a git repository to attach",
    });
    if (typeof picked !== "string") return;
    await run(() => attachRepoToProject(projectId, picked));
  }

  async function onRelocate(path: string) {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Select the repository's new location",
    });
    if (typeof picked !== "string" || picked === path) return;
    await run(() => relocateProject(path, picked));
  }

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Repositories</h2>
        <p className="ps-section-lead text-sm">
          Where this project lives on disk. Attach more repositories to group a frontend, a backend,
          and more under one project — they can live anywhere on your machine.
        </p>
      </header>

      <div className="ps-repo-list">
        {repos.map((r, i) => (
          <RepoRow
            key={r.path}
            repo={r}
            primary={i === 0 && multi}
            detachable={multi}
            busy={busy}
            onRelocate={() => void onRelocate(r.path)}
            onDetach={() => void run(() => detachRepoFromProject(projectId, r.path))}
          />
        ))}
      </div>

      <button type="button" className="ps-btn ps-repo-add" disabled={busy} onClick={onAttach}>
        Attach repository…
      </button>
      <p className="ps-hint text-xs">
        Labels name each repository inside the project. New agents currently start in the primary
        repository. Detaching and relocating never touch the folder on disk.
      </p>

      {error && <div className="ps-error text-sm">{error}</div>}
    </section>
  );
}

/** One attached repo: an editable label over the on-disk path, plus relocate
 *  and (for multi-repo projects) detach actions. */
function RepoRow({
  repo,
  primary,
  detachable,
  busy,
  onRelocate,
  onDetach,
}: {
  repo: ProjectRef;
  primary: boolean;
  detachable: boolean;
  busy: boolean;
  onRelocate: () => void;
  onDetach: () => void;
}) {
  const setRepoLabel = useAppStore((s) => s.setRepoLabel);
  const saved = repo.label ?? "";
  const [label, setLabel] = useState(saved);

  async function saveLabel() {
    if (label.trim() === saved) return;
    try {
      await setRepoLabel(repo.path, label);
      // The store round-trips the trimmed value (or "" when cleared); mirror it
      // so the field matches what was persisted.
      setLabel(label.trim());
    } catch {
      setLabel(saved); // revert on failure; section-level errors cover the rest
    }
  }

  return (
    <div className="ps-repo-row flex-center">
      <Icon name="folder" size={14} />
      <div className="ps-repo-main">
        <div className="ps-repo-toprow flex-center">
          <input
            className="ps-repo-label text-sm"
            value={label}
            placeholder={basename(repo.path)}
            spellCheck={false}
            autoComplete="off"
            aria-label={`Label for ${repo.path}`}
            onChange={(e) => setLabel(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                (e.target as HTMLInputElement).blur();
              } else if (e.key === "Escape") {
                e.stopPropagation();
                setLabel(saved);
                (e.target as HTMLInputElement).blur();
              }
            }}
            onBlur={() => void saveLabel()}
          />
          {primary && <span className="ps-repo-primary text-xs">primary</span>}
        </div>
        <div className="ps-repo-path mono text-xs truncate" title={repo.path}>
          {repo.path}
        </div>
      </div>
      <button type="button" className="ps-repo-act text-sm" disabled={busy} onClick={onRelocate}>
        Change…
      </button>
      {detachable && (
        <button
          type="button"
          className="ps-repo-x iflex-center tip"
          data-tip="Detach from project"
          aria-label={`Detach ${repo.path}`}
          disabled={busy}
          onClick={onDetach}
        >
          <Icon name="close" size={12} />
        </button>
      )}
    </div>
  );
}
