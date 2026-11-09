import type { FileStatus } from "@/api";
import { Badge } from "@/components/ui/Badge";
import { basename } from "@/util/format";
import type { Step, StepAnchor } from "./model";

interface Props {
  step: Step;
  files: FileStatus[];
  currentPath: string;
  onOpen: (anchor: StepAnchor) => void;
}

/** The step's files, each once — a file the step points at twice opens at the
 *  first place. A file not among the changes in view can't be opened. */
export function StepChips({ step, files, currentPath, onOpen }: Props) {
  const byPath = new Map<string, StepAnchor>();
  for (const anchor of step.anchors) if (!byPath.has(anchor.path)) byPath.set(anchor.path, anchor);

  return (
    <div className="wt-chips">
      {[...byPath.values()].map((anchor) => {
        const inView = files.some((f) => f.path === anchor.path);
        return (
          <button
            key={anchor.path}
            type="button"
            className={`wt-chip iflex-center text-xs${anchor.path === currentPath ? " current" : ""}`}
            title={inView ? anchor.path : `${anchor.path} — not in the current changes`}
            disabled={!inView}
            onClick={() => onOpen(anchor)}
          >
            {basename(anchor.path)}
          </button>
        );
      })}
      {step.stale && (
        <Badge variant="warn" hint="None of this step's files are in the current changes">
          stale
        </Badge>
      )}
    </div>
  );
}
