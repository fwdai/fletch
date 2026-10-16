import type { FileStatus } from "@/api";
import { Icon } from "@/components/Icon";
import { statusLetter } from "@/util/diff";

/** The uncommitted files, one row each, under the ladder's Changes row (which
 *  is their heading). Rows open the file's diff when `onOpen` is given; without
 *  it the list is read-only. */
export function ChangesList({
  files,
  onOpen,
}: {
  files: FileStatus[];
  onOpen?: (path: string) => void;
}) {
  return (
    <div className="git-file-list">
      {files.map((f) => (
        <div
          key={f.path}
          role={onOpen ? "button" : undefined}
          className={`git-file flex-center text-sm ${onOpen ? "openable" : ""}`}
          onClick={onOpen ? () => onOpen(f.path) : undefined}
        >
          <span className={`gs text-xs ${f.kind}`}>{statusLetter(f.kind)}</span>
          <span className="gn">{f.path}</span>
          <span className="gx text-xs">
            {f.additions > 0 && <span className="add">+{f.additions}</span>}
            {f.deletions > 0 && <span className="rem">−{f.deletions}</span>}
          </span>
          {onOpen && <Icon name="chevR" size={11} className="gc" />}
        </div>
      ))}
    </div>
  );
}
