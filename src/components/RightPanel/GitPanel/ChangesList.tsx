import type { FileStatus } from "../../../api";
import { Icon } from "../../Icon";
import { IconButton } from "../../ui/IconButton";

/** Status letter for the file badge — matches CSS `.gs.<kind>` selectors. */
function kindLabel(kind: FileStatus["kind"]): string {
  switch (kind) {
    case "modified":
      return "M";
    case "added":
      return "A";
    case "deleted":
      return "D";
    case "renamed":
      return "R";
    case "untracked":
      return "?";
    case "conflicted":
      return "!";
    default:
      return "?";
  }
}

/** The uncommitted-changes file list (changes / conflicts states). */
export function ChangesList({
  files,
  selected,
  onSelect,
  onRefresh,
}: {
  files: FileStatus[];
  selected: string | null;
  onSelect: (path: string) => void;
  onRefresh: () => void;
}) {
  return (
    <div className="git-files">
      <div className="git-files-h flex-center text-2xs">
        <span>
          Changes <span className="n">{files.length}</span>
        </span>
        <div className="actions">
          <IconButton tip="Refresh" size="xs" onClick={onRefresh}>
            <Icon name="refresh" />
          </IconButton>
        </div>
      </div>
      <div className="git-file-list">
        {files.map((f) => (
          <div
            key={f.path}
            className={`git-file flex-center text-sm ${selected === f.path ? "active" : ""}`}
            onClick={() => onSelect(f.path)}
          >
            <span className={`gs text-2xs ${f.kind}`}>{kindLabel(f.kind)}</span>
            <span className="gn">{f.path}</span>
            <span className="gx text-2xs">
              {f.additions > 0 && <span className="add">+{f.additions}</span>}
              {f.deletions > 0 && <span className="rem">−{f.deletions}</span>}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
