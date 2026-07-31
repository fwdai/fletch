import { useState } from "react";
import { Button } from "@/components/ui/Button";
import { ModalBody } from "@/components/ui/Modal";
import { Spinner } from "@/components/ui/Spinner";
import { TextInput } from "@/components/ui/TextInput";
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
    <ModalBody>
      {pasteMode ? (
        <div className="modal-field">
          <label className="modal-label text-sm">Repository URL or owner/repo</label>
          <TextInput
            autoFocus
            placeholder="https://github.com/owner/repo  ·  owner/repo"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
          />
          <Button variant="link" size="sm" onClick={() => setPasteMode(false)}>
            Pick from my repositories instead
          </Button>
        </div>
      ) : (
        <>
          <RepoList selected={selected} onSelect={setSelected} />
          <Button variant="link" size="sm" onClick={() => setPasteMode(true)}>
            Paste a URL instead
          </Button>
        </>
      )}

      <DestRow parent={parent} onPick={pickParent} name={parsed.name} />

      {error && <div className="modal-error text-sm">{error}</div>}

      <div className="modal-actions">
        <Button variant="primary" size="lg" disabled={!canClone} onClick={onClone}>
          {busy ? (
            <>
              <Spinner /> Cloning…
            </>
          ) : (
            "Clone repository"
          )}
        </Button>
      </div>
    </ModalBody>
  );
}
