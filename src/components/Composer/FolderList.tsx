import type { FolderGrant } from "@/api";
import { Icon } from "@/components/Icon";
import { baseName } from "@/util/paths";

interface Props {
  folders: FolderGrant[];
  /** Folders the agent already has (`AgentRecord.folder_grants`), changed at
   *  once, rather than chips the next send grants. */
  granted?: boolean;
  /** Flip a folder between read-only and writable. */
  onToggle: (folder: FolderGrant) => void;
  /** Drop a chip, or take a granted folder back. */
  onRemove: (path: string) => void;
}

/** Folder chips: a folder given to the agent live, at its own path. Each shows
 *  its mode and flips it on click — read-only by default, writable when the
 *  agent should write there too. */
export function FolderList({ folders, granted = false, onToggle, onRemove }: Props) {
  return (
    <div className="composer-attachments">
      {folders.map((folder) => {
        const name = baseName(folder.path);
        const access = folder.writable ? "read and change" : "read";
        const tip = granted
          ? `${folder.path} — the agent can ${access} this folder`
          : `${folder.path} — once you send, the agent can ${access} this folder`;
        return (
          <span
            key={folder.path}
            className={`attachment folder-grant iflex-center text-sm${granted ? " is-granted" : ""}`}
            title={tip}
          >
            <Icon name={folder.writable ? "folder" : "folderLock"} size={12} />
            <span className="attachment-name truncate">{name}</span>
            <button
              type="button"
              className="folder-grant-mode"
              aria-label={`${name}: ${folder.writable ? "writable" : "read-only"} — click to change`}
              onClick={() => onToggle(folder)}
            >
              {folder.writable ? "can write" : "read-only"}
            </button>
            <button
              type="button"
              className="attachment-remove iflex-center"
              aria-label={granted ? `Take back ${name}` : `Remove ${name}`}
              onClick={() => onRemove(folder.path)}
            >
              <Icon name="close" size={11} />
            </button>
          </span>
        );
      })}
    </div>
  );
}
