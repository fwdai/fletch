import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";

interface Props {
  projectId: string;
  /** Current display name (from the store), used as the edit baseline. */
  currentName: string;
  /** The repo's location on disk — the group's stable id. */
  repoPath: string;
}

/** Project identity: a custom display name (independent of the folder) and the
 *  on-disk location. Renaming only relabels the project; changing the location
 *  repoints Fletch at a folder the user has already moved. */
export function GeneralSection({ projectId, currentName, repoPath }: Props) {
  const renameProject = useAppStore((s) => s.renameProject);
  const relocateProject = useAppStore((s) => s.relocateProject);

  const [name, setName] = useState(currentName);
  const [saving, setSaving] = useState(false);
  const [relocating, setRelocating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const trimmed = name.trim();
  const dirty = trimmed.length > 0 && trimmed !== currentName;

  async function saveName() {
    if (!dirty || saving) return;
    setSaving(true);
    setError(null);
    try {
      await renameProject(projectId, trimmed);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  async function changeLocation() {
    if (relocating) return;
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Select the project's new location",
    });
    if (typeof picked !== "string" || picked === repoPath) return;
    setRelocating(true);
    setError(null);
    try {
      await relocateProject(repoPath, picked);
    } catch (e) {
      setError(String(e));
    } finally {
      setRelocating(false);
    }
  }

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">General</h2>
        <p className="ps-section-lead text-sm">
          The project&rsquo;s display name and where Fletch looks for it on disk.
        </p>
      </header>

      <div className="ps-field">
        <label className="ps-label text-sm" htmlFor="ps-name">
          Name
        </label>
        <div className="ps-name-row">
          <input
            id="ps-name"
            className="ps-input text-base"
            value={name}
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void saveName();
              } else if (e.key === "Escape") {
                // Cancel the edit; don't let it bubble up and close the modal.
                e.stopPropagation();
                setName(currentName);
                (e.target as HTMLInputElement).blur();
              }
            }}
            onBlur={() => void saveName()}
          />
          <button
            type="button"
            className="ps-btn"
            disabled={!dirty || saving}
            onClick={() => void saveName()}
          >
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
        <p className="ps-hint text-xs">Shown in the sidebar. Independent of the folder name.</p>
      </div>

      <div className="ps-field">
        <label className="ps-label text-sm">Location</label>
        <button
          type="button"
          className="ps-path-row flex-center"
          disabled={relocating}
          onClick={() => void changeLocation()}
        >
          <Icon name="folder" size={14} />
          <span className="ps-path-val mono text-base truncate">{repoPath}</span>
          <span className="ps-path-change text-sm">{relocating ? "Moving…" : "Change…"}</span>
        </button>
        <p className="ps-hint text-xs">
          Moved the folder? Point Fletch at its new location. Running agents keep their existing
          worktrees — only new agents use the new path.
        </p>
      </div>

      {error && <div className="ps-error text-sm">{error}</div>}
    </section>
  );
}
