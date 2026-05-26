import { open } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "../../store";
import { Icon, type IconName } from "../Icon";
import { Scrim } from "../ui/Scrim";

/** Choose how to add a project to the sidebar. "Folder" is the only
 *  path currently wired; the other options are placeholders for
 *  future GitHub clone / new-repo flows. */
export function NewProjectPopover({ onClose }: { onClose: () => void }) {
  const addWorkspaceRepo = useAppStore((s) => s.addWorkspaceRepo);

  async function onOpenFolder() {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Select a git repository",
    });
    if (typeof picked === "string") {
      await addWorkspaceRepo(picked);
    }
    onClose();
  }

  return (
    <>
      <Scrim onClose={onClose} zIndex={290} />
      <div className="np-pop">
        <Item
          icon="folder"
          title="Open a folder"
          subtitle="Local repo on your machine"
          onClick={onOpenFolder}
        />
        <Item
          icon="github"
          title="Clone from GitHub"
          subtitle="Coming soon"
          onClick={onClose}
        />
        <Item
          icon="sparkle"
          title="Create new project"
          subtitle="Coming soon"
          onClick={onClose}
        />
      </div>
    </>
  );
}

interface ItemProps {
  icon: IconName;
  title: string;
  subtitle: string;
  onClick: () => void;
}

function Item({ icon, title, subtitle, onClick }: ItemProps) {
  return (
    <button className="np-item" onClick={onClick}>
      <div className="np-icon">
        <Icon name={icon} />
      </div>
      <div className="np-text">
        <div className="np-t">{title}</div>
        <div className="np-s">{subtitle}</div>
      </div>
    </button>
  );
}
