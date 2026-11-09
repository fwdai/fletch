// One row per changed file — the Code panel's Changes list and the Git panel's
// uncommitted list. Everything beyond the row itself is opt-in, so the Git
// panel gets a plain list by passing only `files` and `onOpen`.
import type { FileStatus } from "@/api";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { statusLetter } from "@/util/diff";
import { basename, parentDir } from "@/util/format";

interface ChangedFileListProps {
  files: FileStatus[];
  /** Rows open the file's diff when given; without it the list is read-only. */
  onOpen?: (path: string) => void;
  /** The file last opened, marked so the list keeps the reader's place. */
  activePath?: string | null;
  /** The file the agent is editing now. */
  liveFile?: string | null;
  /** Viewed state; the check is drawn only when both are given. */
  isViewed?: (f: FileStatus) => boolean;
  onToggleViewed?: (f: FileStatus) => void;
}

export function ChangedFileList({
  files,
  onOpen,
  activePath,
  liveFile,
  isViewed,
  onToggleViewed,
}: ChangedFileListProps) {
  return (
    <div className="cfl">
      {files.map((f) => {
        const viewed = isViewed?.(f) ?? false;
        const dir = parentDir(f.path);
        const cls = [
          "cfl-row flex-center text-sm",
          onOpen ? "openable" : "",
          f.path === activePath ? "active" : "",
          viewed ? "viewed" : "",
        ];
        return (
          <div
            key={f.path}
            role={onOpen ? "button" : undefined}
            className={cls.filter(Boolean).join(" ")}
            onClick={onOpen ? () => onOpen(f.path) : undefined}
            title={f.path}
          >
            <StatusBadge kind={f.kind} />
            <span className="cfl-name">
              {dir && <span className="cfl-dir">{dir}/</span>}
              <span className="cfl-base">{basename(f.path)}</span>
            </span>
            <span className="cfl-stats text-xs">
              {f.additions > 0 && <span className="add">+{f.additions}</span>}
              {f.deletions > 0 && <span className="rem">−{f.deletions}</span>}
            </span>
            {f.path === liveFile && <span className="cfl-live" title="The agent is editing this" />}
            {isViewed && onToggleViewed && (
              <IconButton
                size="xs"
                className={`cfl-viewed ${viewed ? "on" : ""}`}
                tip={viewed ? "Viewed — click to unmark" : "Mark as viewed"}
                aria-pressed={viewed}
                onClick={(e) => {
                  e.stopPropagation();
                  onToggleViewed(f);
                }}
              >
                <Icon name="check" />
              </IconButton>
            )}
            {onOpen && <Icon name="chevR" size={11} className="cfl-chev" />}
          </div>
        );
      })}
    </div>
  );
}

/** The one-letter git status, tinted like the file tree's badges. */
function StatusBadge({ kind }: { kind: FileStatus["kind"] }) {
  const letter = statusLetter(kind);
  return <span className={`fp-badge iflex-center s-${letter.toLowerCase()}`}>{letter}</span>;
}
