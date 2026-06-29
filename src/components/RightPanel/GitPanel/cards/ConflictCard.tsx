import type { FileStatus } from "../../../../api";
import { Icon } from "../../../Icon";

export function ConflictCard({ files }: { files: FileStatus[] }) {
  const conflicted = files.filter((f) => f.kind === "conflicted");
  const first = conflicted[0]?.path ?? "";
  const rest = conflicted.length - 1;
  return (
    <div className="git-banner flex-center att text-base">
      <Icon name="merge" size={14} />
      <span>
        Conflicts in <span className="mono text-xs">{first}</span>
        {rest > 0 && ` and ${rest} more`}. The agent can reconcile them.
      </span>
    </div>
  );
}
