import { FileIcon } from "../FileIcon";
import { Icon } from "../../Icon";

interface ViewerHeaderProps {
  name: string;
  dir: string;
  status: string | null;
  dirty: boolean;
  onBack: () => void;
  actions?: React.ReactNode;
}

/** Header bar shown above a file's contents (loading, error, and editor
 *  states all share it): back button, breadcrumb, git badge, and optional
 *  action buttons supplied by the editor. */
export function ViewerHeader({ name, dir, status, dirty, onBack, actions }: ViewerHeaderProps) {
  const st = status ? status.toLowerCase() : "";
  return (
    <div className="fp-viewer-h">
      <button className="fp-back" title="Back to files" onClick={onBack}>
        <Icon name="chevL" size={13} />
      </button>
      <FileIcon name={name} />
      <div className="fp-crumb">
        {dir && <span className="fp-crumb-dir">{dir}/</span>}
        <span className="fp-crumb-file">{name}</span>
        {dirty && <span className="fp-crumb-dot" title="Unsaved changes"></span>}
      </div>
      {status && <span className={`fp-badge s-${st}`}>{status}</span>}
      {actions}
    </div>
  );
}
