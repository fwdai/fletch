import { Icon } from "@/components/Icon";
import { plural } from "@/util/format";
import type { WalkthroughSource } from "./useWalkthrough";

interface Props {
  count: number;
  source: WalkthroughSource;
  /** The reader has been on the walkthrough before: the way in is back to
   *  where they were. */
  started: boolean;
  onStart: () => void;
}

/** Over the file list: that there is a walkthrough, where it came from, and the
 *  way in. */
export function StartCard({ count, source, started, onStart }: Props) {
  return (
    <div className="wt wt-row">
      <Icon name="map" size={13} className="wt-icon" />
      <span className="wt-label text-sm">Walkthrough · {plural(count, "step")}</span>
      <span className="wt-hint text-xs truncate">
        {source === "tour" ? "from the agent's notes" : "from the transcript"}
      </span>
      <button
        type="button"
        className="wt-btn iflex-center text-xs tip"
        data-tip="Then j / k to step through"
        onClick={onStart}
      >
        {started ? "Resume" : "Start"}
        <Icon name="arrowR" size={11} />
      </button>
    </div>
  );
}
