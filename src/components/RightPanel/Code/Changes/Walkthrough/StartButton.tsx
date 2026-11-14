import { Icon } from "@/components/Icon";
import { plural } from "@/util/format";
import type { WalkthroughSource } from "./useWalkthrough";

/** The way into the walkthrough while the file list is showing. */
export interface WalkthroughStart {
  count: number;
  source: WalkthroughSource;
  /** The reader has been on the walkthrough before: the way in is back to
   *  where they were. */
  started: boolean;
  onStart: () => void;
}

/** That there is a walkthrough, and the way in — one pill on the Changes
 *  header's row. Where the steps came from is the tooltip's to say. */
export function StartButton({ count, source, started, onStart }: WalkthroughStart) {
  const from = source === "tour" ? "From the agent's notes" : "From the transcript";
  return (
    <button
      type="button"
      className="wt-btn wt-start iflex-center text-xs tip"
      data-tip={`${from} · then j / k to step through`}
      onClick={onStart}
    >
      <Icon name="map" size={12} className="wt-icon" />
      {started ? "Resume walkthrough" : `Walkthrough · ${plural(count, "step")}`}
      <Icon name="arrowR" size={11} />
    </button>
  );
}
