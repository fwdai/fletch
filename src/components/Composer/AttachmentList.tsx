import { Icon } from "@/components/Icon";

function basename(path: string) {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

interface Props {
  paths: string[];
  /** Omit for a read-only list (e.g. chips on a sent message); when present
   *  each chip gets a remove button (the composer's staged-files view). */
  onRemove?: (path: string) => void;
  className?: string;
}

/** File chips: a file icon + filename with the absolute path as tooltip.
 *  Used both for files staged in the composer (with `onRemove`) and read-only
 *  on a sent message. Paths are folded into the message text on send so the
 *  agent reads each via its tools. */
export function AttachmentList({ paths, onRemove, className = "composer-attachments" }: Props) {
  return (
    <div className={className}>
      {paths.map((path) => (
        <span key={path} className="attachment iflex-center text-sm" title={path}>
          <Icon name="file" size={12} />
          <span className="attachment-name truncate">{basename(path)}</span>
          {onRemove && (
            <button
              type="button"
              className="attachment-remove iflex-center"
              aria-label={`Remove ${basename(path)}`}
              onClick={() => onRemove(path)}
            >
              <Icon name="close" size={11} />
            </button>
          )}
        </span>
      ))}
    </div>
  );
}
