import type { ReactNode } from "react";
import { Icon } from "@/components/Icon";
import { FileIcon } from "@/components/RightPanel/FileIcon";

interface ViewerHeaderProps {
  name: string;
  dir: string;
  status: string | null;
  dirty: boolean;
  onBack: () => void;
  /** Tooltip on the back button — names the list it returns to. */
  backTitle?: string;
  actions?: ReactNode;
}

/** Header bar shown above a file's contents (loading, error, and editor
 *  states all share it): back button, breadcrumb, git badge, and optional
 *  action buttons supplied by the editor. */
export function ViewerHeader({
  name,
  dir,
  status,
  dirty,
  onBack,
  backTitle = "Back to files",
  actions,
}: ViewerHeaderProps) {
  const st = status ? status.toLowerCase() : "";
  return (
    <div className="fp-viewer-h flex-center">
      <button className="fp-back iflex-center" title={backTitle} onClick={onBack}>
        <Icon name="chevL" size={13} />
      </button>
      <FileIcon name={name} />
      <div className="fp-crumb text-sm">
        {dir && <span className="fp-crumb-dir">{dir}/</span>}
        <span className="fp-crumb-file">{name}</span>
        {dirty && <span className="fp-crumb-dot" title="Unsaved changes"></span>}
      </div>
      {status && <span className={`fp-badge iflex-center s-${st}`}>{status}</span>}
      {actions}
    </div>
  );
}
