import { Icon } from "@/components/Icon";
import { plural } from "@/util/format";
import type { WalkthroughSource } from "./useWalkthrough";

interface Props {
  count: number;
  source: WalkthroughSource;
  /** Absent when no step has a file in view to open. */
  onStart?: () => void;
}

/** Over the file list: that there is a walkthrough, where it came from, and the
 *  way in. */
export function StartCard({ count, source, onStart }: Props) {
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
        disabled={!onStart}
        onClick={onStart}
      >
        Start
        <Icon name="arrowR" size={11} />
      </button>
    </div>
  );
}
