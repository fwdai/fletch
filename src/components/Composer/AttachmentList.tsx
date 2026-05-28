import { Icon } from "../Icon";

function basename(path: string) {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

interface Props {
  paths: string[];
  onRemove: (path: string) => void;
}

/** Preview chips for files staged in the composer. v1 shows a file icon
 *  + filename with the absolute path as tooltip; the paths are folded
 *  into the message text on send so the agent reads each via its tools. */
export function AttachmentList({ paths, onRemove }: Props) {
  return (
    <div className="composer-attachments">
      {paths.map((path) => (
        <span key={path} className="attachment" title={path}>
          <Icon name="file" size={12} />
          <span className="attachment-name">{basename(path)}</span>
          <button
            type="button"
            className="attachment-remove"
            aria-label={`Remove ${basename(path)}`}
            onClick={() => onRemove(path)}
          >
            <Icon name="close" size={11} />
          </button>
        </span>
      ))}
    </div>
  );
}
