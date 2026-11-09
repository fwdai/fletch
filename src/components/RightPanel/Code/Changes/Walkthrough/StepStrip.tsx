import type { FileStatus } from "@/api";
import { Icon } from "@/components/Icon";
import type { Step, StepAnchor } from "./model";
import { StepChips } from "./StepChips";

interface Props {
  step: Step;
  /** 0-based position of `step`. */
  index: number;
  total: number;
  files: FileStatus[];
  currentPath: string;
  onOpen: (anchor: StepAnchor) => void;
  onPrev: () => void;
  onNext: () => void;
}

/** Over the diff: the step being read, its files, and the way on. */
export function StepStrip({
  step,
  index,
  total,
  files,
  currentPath,
  onOpen,
  onPrev,
  onNext,
}: Props) {
  return (
    <div className="wt wt-strip">
      <div className="wt-head">
        <span className="wt-count text-xs">
          Step {index + 1} of {total}
        </span>
        <div className="wt-nav">
          <button
            type="button"
            className="wt-btn iflex-center text-xs tip"
            data-tip="Previous step · k"
            disabled={index === 0}
            onClick={onPrev}
          >
            <Icon name="chevL" size={11} />
            Prev
          </button>
          <button
            type="button"
            className="wt-btn iflex-center text-xs tip"
            data-tip="Next step · j"
            disabled={index === total - 1}
            onClick={onNext}
          >
            Next
            <Icon name="chevR" size={11} />
          </button>
        </div>
      </div>
      {step.title && <div className="wt-title text-sm">{step.title}</div>}
      <p className="wt-note text-sm">{step.note}</p>
      <StepChips step={step} files={files} currentPath={currentPath} onOpen={onOpen} />
    </div>
  );
}
