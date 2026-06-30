import { open } from "@tauri-apps/plugin-dialog";
import { Icon, type IconName } from "@/components/Icon";
import type { NewProjectMode } from "@/components/NewProject";
import { Scrim } from "@/components/ui/Scrim";
import { useAppStore } from "@/store";

/** Choose how to add a project to the sidebar: open a local folder, clone
 *  from GitHub, or create a new repo. The latter two open the New Project
 *  modal (hosted by the sidebar) via `onChoose`. */
export function NewProjectPopover({
  onClose,
  onChoose,
}: {
  onClose: () => void;
  onChoose: (mode: NewProjectMode) => void;
}) {
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
          subtitle="Pick a repo or paste a URL"
          onClick={() => onChoose("clone")}
        />
        <Item
          icon="sparkle"
          title="Create new project"
          subtitle="New repo, local + on GitHub"
          onClick={() => onChoose("create")}
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
