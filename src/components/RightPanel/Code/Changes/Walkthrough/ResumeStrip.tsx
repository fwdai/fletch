interface Props {
  /** 1-indexed step the button goes to. */
  step: number;
  /** The reader has been on the walkthrough before. */
  started: boolean;
  onResume: () => void;
}

/** Over a diff no step points at: the reader is off the walkthrough (or has
 *  not begun it); offer the way on. */
export function ResumeStrip({ step, started, onResume }: Props) {
  return (
    <div className="wt wt-row">
      <span className="wt-hint text-xs">Not part of the walkthrough</span>
      <button
        type="button"
        className="wt-btn iflex-center text-xs tip"
        data-tip={started ? `Back to step ${step}` : "Start at step 1 · then j / k"}
        onClick={onResume}
      >
        {started ? "Resume" : "Start"}
      </button>
    </div>
  );
}
