import { Icon } from "@desktop/components/Icon";
import { useRef } from "react";
import { baseName } from "../lib/paths";
import type { StagedAttachment } from "./useAttachments";

/** Chips for files staged in a composer: a spinner while each uploads, and a
 *  remove control. Renders nothing for an empty list, so it can sit in the
 *  tree unconditionally. */
export function StagedChips({
  items,
  onRemove,
  className = "atts",
}: {
  items: StagedAttachment[];
  onRemove: (id: string) => void;
  className?: string;
}) {
  if (items.length === 0) return null;
  return (
    <div className={className}>
      {items.map((a) => (
        <span key={a.id} className={`att${a.path ? "" : " up"}`} title={a.path ?? a.name}>
          <Icon name={a.path ? "file" : "refresh"} size={12} />
          <span className="nm">{a.name}</span>
          <button
            type="button"
            className="rm"
            aria-label={`Remove ${a.name}`}
            onClick={() => onRemove(a.id)}
          >
            <Icon name="close" size={12} />
          </button>
        </span>
      ))}
    </div>
  );
}

/** Read-only chips on a sent message: the file names, from the host paths. */
export function SentChips({ paths }: { paths: string[] | undefined }) {
  if (!paths || paths.length === 0) return null;
  return (
    <div className="atts">
      {paths.map((p) => (
        <span key={p} className="att" title={p}>
          <Icon name="file" size={12} />
          <span className="nm">{baseName(p)}</span>
        </span>
      ))}
    </div>
  );
}

/** The system picker behind an attach control: a hidden file input the
 *  button clicks. On iOS this is the Photos / Camera / Files sheet, with no
 *  plugin and no permission prompt of its own. Resetting `value` after each
 *  pick lets the same file be chosen twice in a row. */
export function AttachButton({
  onPick,
  className,
  label = "Attach a file",
  disabled,
  children,
}: {
  onPick: (files: FileList) => void;
  className: string;
  label?: string;
  disabled?: boolean;
  children: React.ReactNode;
}) {
  const input = useRef<HTMLInputElement>(null);
  return (
    <>
      <input
        ref={input}
        type="file"
        multiple
        hidden
        onChange={(e) => {
          if (e.target.files?.length) onPick(e.target.files);
          e.target.value = "";
        }}
      />
      <button
        type="button"
        className={className}
        aria-label={label}
        disabled={disabled}
        onClick={() => input.current?.click()}
      >
        {children}
      </button>
    </>
  );
}
