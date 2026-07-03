import { useState } from "react";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { parseRepoSpec } from "@/util/repoSpec";
import { RepoList } from "./RepoList";
import { ConnectGitHub, DestRow, type NewProjectShared } from "./shared";

/** Clone an existing GitHub repo — pick from the user's repos or paste a
 *  URL / owner-repo spec. */
export function CloneView({ shared, onDone }: { shared: NewProjectShared; onDone: () => void }) {
  const cloneRepo = useAppStore((s) => s.cloneRepo);
  const { parent, pickParent, gh } = shared;

  const [selected, setSelected] = useState<string | null>(null);
  const [url, setUrl] = useState("");
  const [pasteMode, setPasteMode] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Cloning genuinely needs GitHub — prompt to connect in place. Once
  // connected, `gh.authenticated` flips and this view renders the picker.
  if (!gh?.authenticated) return <ConnectGitHub what="clone a repository" />;

  // The active spec is the pasted URL (when in paste mode) or the selected repo.
  const spec = pasteMode ? url.trim() : (selected ?? "");
  const parsed = parseRepoSpec(spec);
  const canClone = !!parent && parsed.valid && !busy;

  const onClone = async () => {
    if (!canClone) return;
    setBusy(true);
    setError(null);
    try {
      await cloneRepo(spec, parent);
      onDone();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <div className="np-body">
      {pasteMode ? (
        <div className="np-field">
          <label>Repository URL or owner/repo</label>
          <input
            autoFocus
            placeholder="https://github.com/owner/repo  ·  owner/repo"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
          />
          <button className="np-link text-sm" onClick={() => setPasteMode(false)}>
            Pick from my repositories instead
          </button>
        </div>
      ) : (
        <>
          <RepoList selected={selected} onSelect={setSelected} />
          <button className="np-link text-sm" onClick={() => setPasteMode(true)}>
            Paste a URL instead
          </button>
        </>
      )}

      <DestRow parent={parent} onPick={pickParent} name={parsed.name} />

      {error && <div className="np-error text-sm">{error}</div>}

      <div className="np-actions">
        <button className="np-primary flex-center text-base" disabled={!canClone} onClick={onClone}>
          {busy ? (
            <>
              <Icon name="refresh" size={13} /> Cloning…
            </>
          ) : (
            "Clone repository"
          )}
        </button>
      </div>
    </div>
  );
}
