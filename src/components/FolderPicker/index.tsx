import { useEffect, useState, useSyncExternalStore } from "react";
import { api } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { Modal, ModalBody, ModalFooter } from "@/components/ui/Modal";
import { TextInput } from "@/components/ui/TextInput";
import { useFolderBrowser } from "@/util/folderBrowser";
import { childPath } from "@/util/paths";
import { folderRequest, resolveFolderRequest, subscribeFolderRequest } from "./pickFolder";

export { pickFolder } from "./pickFolder";

/** The one folder picker for the whole app, mounted at the root. Renders only
 *  while a `pickFolder` call is waiting on it — which only happens on a remote
 *  environment, where the native dialog cannot see the disk being browsed. */
export function FolderPickerHost() {
  const request = useSyncExternalStore(subscribeFolderRequest, folderRequest);
  if (!request) return null;
  return (
    <FolderPicker
      title={request.title}
      start={request.start}
      onDone={resolveFolderRequest}
      // A fresh browser per request: the next one starts where it asked to,
      // not where the last one was left.
      key={`${request.title}:${request.start}`}
    />
  );
}

/** Browse the host's disk over `list_dir` and answer with an absolute path.
 *  Directories only, since every caller is choosing a folder. Sits on the
 *  overlay layer so it can be opened from inside another modal (New Project). */
function FolderPicker({
  title,
  start,
  onDone,
}: {
  title: string;
  start: string;
  onDone: (path: string | null) => void;
}) {
  const {
    base,
    up,
    shown,
    hiddenCount,
    showHidden,
    setShowHidden,
    setPath,
    listing,
    loading,
    error,
  } = useFolderBrowser({ listDir: api.listDir, start, keepListingOnError: true });

  // The field is editable, so it carries its own draft: typing must not read as
  // a folder change. It follows the browser wherever a click lands it, and
  // Enter is what hands a typed path back.
  const [draft, setDraft] = useState(start);
  useEffect(() => setDraft(base), [base]);

  return (
    <Modal icon="folder" title={title} layer="overlay" onClose={() => onDone(null)}>
      <ModalBody className="fp-body">
        <div className="fp-path flex-center">
          <IconButton
            size="lg"
            variant="outline"
            tip="Up one folder"
            disabled={!up}
            onClick={() => up && setPath(up)}
          >
            <Icon name="arrowUp" />
          </IconButton>
          <TextInput
            mono
            aria-label="Folder path"
            spellCheck={false}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key !== "Enter") return;
              e.preventDefault();
              setPath(draft.trim());
            }}
          />
        </div>

        <div className="fp-list">
          {shown.map((entry) => (
            <button
              type="button"
              key={entry.name}
              className="fp-row flex-center"
              onClick={() => setPath(childPath(base, entry.name))}
            >
              <Icon name="folder" size={14} />
              <span className="fp-name text-base">{entry.name}</span>
              {/* A folder with a `.git` in it is what both callers are looking
                  for, so say which ones those are rather than make the user
                  open each one. */}
              {entry.is_repo && <span className="fp-repo text-sm">repo</span>}
              <Icon name="chevR" size={12} />
            </button>
          ))}
          {shown.length === 0 && (
            <div className="fp-empty text-sm">
              {loading ? "Reading…" : error ? "Couldn't read this folder" : "No folders here"}
            </div>
          )}
          {/* The host stops reading a huge directory at its cap, so say that
              this is a slice rather than let a missing folder read as absent.
              Typing the path is the way past it. */}
          {listing?.truncated && (
            <div className="fp-empty text-sm">Too many entries to list — type a path above</div>
          )}
        </div>

        {hiddenCount > 0 && (
          <Button variant="link" size="sm" onClick={() => setShowHidden(!showHidden)}>
            {showHidden ? "Hide" : "Show"} {hiddenCount} hidden
          </Button>
        )}

        {/* The listing under it is the last one that read, so the error says
            what failed without taking the user's place away. */}
        {error && <div className="modal-error text-sm">{error}</div>}
      </ModalBody>

      <ModalFooter className="fp-foot">
        <span className="fp-base mono text-sm" title={base}>
          {base}
        </span>
        <Button variant="ghost" onClick={() => onDone(null)}>
          Cancel
        </Button>
        <Button variant="primary" disabled={!listing} onClick={() => onDone(base)}>
          Use this folder
        </Button>
      </ModalFooter>
    </Modal>
  );
}
