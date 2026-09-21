import { useState } from "react";
import type { ProjectRef } from "@/api";
import { pickFolder } from "@/components/FolderPicker";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { basename } from "@/util/format";

/** The repositories field of the General section: where the project lives on
 *  disk. Each repo row carries its own path, relocate action, and an optional
 *  label ("Frontend", "Gateway") that names it inside the project. One repo is
 *  the common case; attaching more turns the project multi-repo, grouped under
 *  a single sidebar entry. The project itself has no location — repos can live
 *  anywhere. */
export function RepositoriesField({ projectId }: { projectId: string }) {
  const projects = useAppStore((s) => s.workspace?.projects);
  const attachRepoToProject = useAppStore((s) => s.attachRepoToProject);
  const detachRepoFromProject = useAppStore((s) => s.detachRepoFromProject);
  const relocateProject = useAppStore((s) => s.relocateProject);
  // Closed on a host from before the project-settings ops; the section header
  // carries the reason, each dead control repeats it.
  const gate = useGate("projectAdmin");
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

  // Both pickers ask the environment the user is driving: the native dialog on
  // this Mac, the in-app browser over `list_dir` on a paired host — the folder
  // these ops take is one of the host's, not one of ours.
  async function onAttach() {
    const picked = await pickFolder({ title: "Select a git repository to attach" });
    if (!picked) return;
    await run(() => attachRepoToProject(projectId, picked));
  }

  async function onRelocate(path: string) {
    const picked = await pickFolder({
      title: "Select the repository's new location",
      start: path,
    });
    if (!picked || picked === path) return;
    await run(() => relocateProject(path, picked));
  }

  return (
    <div className="ps-field">
      <span className="ps-label text-sm">Repositories</span>

      <div className="ps-repo-list">
        {repos.map((r, i) => (
          <RepoRow
            key={r.path}
            repo={r}
            primary={i === 0 && multi}
            detachable={multi}
            busy={busy}
            gate={gate}
            onRelocate={() => void onRelocate(r.path)}
            onDetach={() => void run(() => detachRepoFromProject(projectId, r.path))}
            onError={setError}
          />
        ))}
      </div>

      <Button
        variant="outline"
        size="lg"
        className="ps-repo-add"
        disabled={busy || gate !== null}
        tip={gate ?? undefined}
        onClick={onAttach}
      >
        Attach repository…
      </Button>
      <p className="ps-hint text-xs">
        Attach more repositories to group a frontend, a backend, and more under one project — they
        can live anywhere on disk. Labels name each repository inside the project; new agents
        currently start in the primary one. Detaching and relocating never touch the folder on disk.
      </p>

      {error && <div className="ps-error text-sm">{error}</div>}
    </div>
  );
}

/** One attached repo: an editable label over the on-disk path, plus relocate
 *  and (for multi-repo projects) detach actions. */
function RepoRow({
  repo,
  primary,
  detachable,
  busy,
  gate,
  onRelocate,
  onDetach,
  onError,
}: {
  repo: ProjectRef;
  primary: boolean;
  detachable: boolean;
  busy: boolean;
  /** Why this host cannot take the row's writes, or null when it can. */
  gate: string | null;
  onRelocate: () => void;
  onDetach: () => void;
  /** Surface a failed save in the field's shared error slot (null clears it) —
   *  a reverted input alone would read as the label silently vanishing. */
  onError: (msg: string | null) => void;
}) {
  const setRepoLabel = useAppStore((s) => s.setRepoLabel);
  const saved = repo.label ?? "";
  const [label, setLabel] = useState(saved);

  async function saveLabel() {
    if (label.trim() === saved || gate) return;
    try {
      await setRepoLabel(repo.path, label);
      // The store round-trips the trimmed value (or "" when cleared); mirror it
      // so the field matches what was persisted.
      setLabel(label.trim());
      onError(null);
    } catch (e) {
      setLabel(saved);
      onError(String(e));
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
            disabled={gate !== null}
            title={gate ?? undefined}
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
      <Button
        variant="link"
        size="sm"
        disabled={busy || gate !== null}
        tip={gate ?? undefined}
        onClick={onRelocate}
      >
        Change…
      </Button>
      {detachable && (
        <IconButton
          size="sm"
          danger
          tip={gate ?? "Detach from project"}
          aria-label={`Detach ${repo.path}`}
          disabled={busy || gate !== null}
          onClick={onDetach}
        >
          <Icon name="close" />
        </IconButton>
      )}
    </div>
  );
}
