import { type ProjectSignal, SIGNAL_LABEL } from "../projectSignal";

/** A project signal as a dot, in the colour the agent rows give that state.
 *  `where` finishes the tooltip: which project, or that it is a hidden one. */
export function SignalDot({ signal, where }: { signal: ProjectSignal; where?: string }) {
  const tip = where ? `${SIGNAL_LABEL[signal]} ${where}` : SIGNAL_LABEL[signal];
  return (
    <span
      className="proj-signal tip"
      role="img"
      data-signal={signal}
      data-tip={tip}
      aria-label={tip}
    />
  );
}
