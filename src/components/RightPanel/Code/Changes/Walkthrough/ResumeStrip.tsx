interface Props {
  /** 1-indexed step Resume returns to. */
  step: number;
  onResume: () => void;
}

/** Over a diff no step points at: the reader wandered off; offer the way back. */
export function ResumeStrip({ step, onResume }: Props) {
  return (
    <div className="wt wt-row">
      <span className="wt-hint text-xs">Not part of the walkthrough</span>
      <button
        type="button"
        className="wt-btn iflex-center text-xs tip"
        data-tip={`Back to step ${step}`}
        onClick={onResume}
      >
        Resume
      </button>
    </div>
  );
}
