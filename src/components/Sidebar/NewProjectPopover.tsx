import { pickFolder } from "@/components/FolderPicker";
import { Icon, type IconName } from "@/components/Icon";
import type { NewProjectMode } from "@/components/NewProject";
import { Scrim } from "@/components/ui/Scrim";
import { useAppStore } from "@/store";
import { activeEntry, useGate } from "@/store/capabilities";

/** Choose how to add a project to the sidebar: open a folder, clone from
 *  GitHub, or create a new repo. The latter two open the New Project modal
 *  (hosted by the sidebar) via `onChoose`. */
export function NewProjectPopover({
  onClose,
  onChoose,
}: {
  onClose: () => void;
  onChoose: (mode: NewProjectMode) => void;
}) {
  const addWorkspaceRepo = useAppStore((s) => s.addWorkspaceRepo);
  const remote = useAppStore((s) => activeEntry(s).kind === "remote");
  const createGate = useGate("createProject");

  async function onOpenFolder() {
    // Close first: the picker is a modal on a remote environment, and this
    // popover's scrim has no business sitting under it.
    onClose();
    const picked = await pickFolder({ title: "Select a git repository" });
    if (picked) await addWorkspaceRepo(picked);
  }

  return (
    <>
      <Scrim onClose={onClose} zIndex={290} />
      <div className="np-pop">
        <Item
          icon="folder"
          title="Open a folder"
          subtitle={remote ? "Repo on the host" : "Local repo on your machine"}
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
          // Gated rather than hidden: the row says why it cannot run here,
          // which is more use than a menu that quietly changes shape.
          subtitle={createGate ?? "New repo, local + on GitHub"}
          disabled={createGate !== null}
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
  disabled?: boolean;
  onClick: () => void;
}

function Item({ icon, title, subtitle, disabled, onClick }: ItemProps) {
  return (
    <button className="np-item" disabled={disabled} onClick={onClick}>
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
