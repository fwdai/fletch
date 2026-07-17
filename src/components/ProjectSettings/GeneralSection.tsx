import { useState } from "react";
import { useAppStore } from "@/store";
import { RepositoriesField } from "./RepositoriesField";

interface Props {
  projectId: string;
  /** Current display name (from the store), used as the edit baseline. */
  currentName: string;
}

/** Project identity: a custom display name (independent of any folder name)
 *  and the repositories the project is made of. The project has no location
 *  of its own — each repo row carries its own path. */
export function GeneralSection({ projectId, currentName }: Props) {
  const renameProject = useAppStore((s) => s.renameProject);

  const [name, setName] = useState(currentName);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const trimmed = name.trim();
  const dirty = trimmed.length > 0 && trimmed !== currentName;

  async function saveName() {
    if (!dirty || saving) return;
    setSaving(true);
    setError(null);
    try {
      await renameProject(projectId, trimmed);
      // Reflect what was actually persisted (trimmed), so the field matches the
      // sidebar instead of keeping the user's raw input with surrounding space.
      setName(trimmed);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">General</h2>
        <p className="ps-section-lead text-sm">
          The project&rsquo;s display name and the repositories it&rsquo;s made of.
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

      <RepositoriesField projectId={projectId} />

      {error && <div className="ps-error text-sm">{error}</div>}
    </section>
  );
}
