import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import { useAppStore } from "../store";

/**
 * Combined repo + base-image picker.
 *
 * Replaces the previous flow that used `window.prompt()` to ask for the
 * base image after the directory picker — `prompt()` returns `null` in
 * Tauri's webview, so the whole flow silently aborted.
 */
export function ChooseRepoDialog({ onClose }: { onClose: () => void }) {
  const workspace = useAppStore((s) => s.workspace);
  const setRepo = useAppStore((s) => s.setRepo);
  const busy = useAppStore((s) => s.busy);
  const lastError = useAppStore((s) => s.lastError);

  const [path, setPath] = useState(workspace?.repo_path ?? "");
  const [baseImage, setBaseImage] = useState(workspace?.base_image ?? "base-dev");
  const [available, setAvailable] = useState<string[] | null>(null);
  const [imagesError, setImagesError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .listBaseImages()
      .then((list) => {
        if (!cancelled) setAvailable(list);
      })
      .catch((e) => {
        if (!cancelled) {
          setAvailable([]);
          setImagesError(String(e));
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

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
    if (!path.trim() || !baseImage.trim()) return;
    await setRepo(path.trim(), baseImage.trim());
    // Only close if the call succeeded. If it failed, `lastError` is set
    // and we keep the modal open so the user can fix and retry.
    if (!useAppStore.getState().lastError) onClose();
  }

  return (
    <>
      <div
        className="backdrop"
        onClick={onClose}
        role="presentation"
      />
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
          </label>
          <label>
            <span>Tart base image</span>
            <input
              value={baseImage}
              onChange={(e) => setBaseImage(e.target.value)}
              placeholder="base-dev"
              list="base-image-options"
            />
            {available && available.length > 0 && (
              <datalist id="base-image-options">
                {available.map((name) => (
                  <option key={name} value={name} />
                ))}
              </datalist>
            )}
            {available === null && <small>Loading available VMs…</small>}
            {available && available.length > 0 && (
              <div className="chips">
                {available.map((name) => (
                  <button
                    key={name}
                    type="button"
                    className={`chip ${baseImage === name ? "active" : ""}`}
                    onClick={() => setBaseImage(name)}
                  >
                    {name}
                  </button>
                ))}
              </div>
            )}
            {available && available.length === 0 && !imagesError && (
              <small className="warn">
                No Tart VMs found on this system. Build a base image first —
                see <code>scripts/build-base-image.md</code>. Short version:
                <br />
                <code>
                  tart clone ghcr.io/cirruslabs/ubuntu:latest base-dev
                </code>
                <br />
                then SSH in and install node + claude code CLI + your
                public key.
              </small>
            )}
            {imagesError && (
              <small className="warn">
                Couldn't list Tart VMs: {imagesError}
              </small>
            )}
          </label>
          {lastError && <div className="formerr">{lastError}</div>}
          <div className="actions">
            <button type="button" onClick={onClose}>
              Cancel
            </button>
            <button
              type="submit"
              className="primary"
              disabled={busy || !path.trim() || !baseImage.trim()}
            >
              {busy ? "Setting…" : "Use this repo"}
            </button>
          </div>
        </form>
      </div>
    </>
  );
}
