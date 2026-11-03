import { useState } from "react";
import type { FolderGrant } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { DropdownItem, DropdownMenu, DropdownSection } from "@/components/ui/Dropdown";
import { IconButton } from "@/components/ui/IconButton";
import { Scrim } from "@/components/ui/Scrim";
import { baseName } from "@/util/paths";

/** The folders the workspace's agent can already use (`AgentRecord.folder_grants`),
 *  beside the attach button: standing access, not chips for the next message.
 *  A folder icon with how many; a click lists them, where each one's mode
 *  flips and its access is taken back. Nothing shows while none is granted. */
export function FolderAccess({
  folders,
  onToggle,
  onRevoke,
}: {
  folders: FolderGrant[];
  /** Flip a folder between read-only and writable. */
  onToggle: (folder: FolderGrant) => void;
  /** Take a folder's access back. */
  onRevoke: (path: string) => void;
}) {
  const [open, setOpen] = useState(false);
  if (folders.length === 0) return null;
  const tip =
    folders.length === 1
      ? `The agent can use ${baseName(folders[0].path)}`
      : `The agent can use ${folders.length} folders`;

  return (
    <span style={{ position: "relative", minWidth: 0 }}>
      <IconButton
        className="composer-action"
        tip={open ? undefined : tip}
        aria-label={tip}
        active={open}
        onClick={() => setOpen((v) => !v)}
      >
        <Icon name="folder" size={15} />
        <span className="folder-access-count">{folders.length}</span>
      </IconButton>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <DropdownMenu
            // Right-anchored, like the issue picker beside it: the trigger sits
            // near the composer's right edge.
            style={{ bottom: "calc(100% + 6px)", right: 0, minWidth: 280, maxWidth: 420 }}
          >
            <DropdownSection>Folders the agent can use</DropdownSection>
            {folders.map((folder) => {
              const name = baseName(folder.path);
              return (
                <DropdownItem key={folder.path} title={folder.path}>
                  <span className="di-i">
                    <Icon name={folder.writable ? "folder" : "folderLock"} size={13} />
                  </span>
                  <span className="di-l">{name}</span>
                  <Button
                    variant="ghost"
                    size="sm"
                    tip={
                      folder.writable
                        ? "Make it read-only"
                        : "Let the agent change files in this folder"
                    }
                    onClick={() => onToggle(folder)}
                  >
                    {folder.writable ? "Can write" : "Read-only"}
                  </Button>
                  <IconButton
                    size="xs"
                    danger
                    tip="Take the agent's access back"
                    aria-label={`Take back ${name}`}
                    onClick={() => onRevoke(folder.path)}
                  >
                    <Icon name="close" size={11} />
                  </IconButton>
                </DropdownItem>
              );
            })}
          </DropdownMenu>
        </>
      )}
    </span>
  );
}
