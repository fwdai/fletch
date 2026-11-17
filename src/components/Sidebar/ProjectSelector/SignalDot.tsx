import { type ProjectSignal, SIGNAL_LABEL } from "../projectSignal";

interface Props {
  signal: ProjectSignal;
  /** No tooltip or accessible name: the control it sits on carries them. */
  decorative?: boolean;
}

export function SignalDot({ signal, decorative = false }: Props) {
  if (decorative) {
    return <span className="proj-signal" data-signal={signal} aria-hidden="true" />;
  }
  const tip = SIGNAL_LABEL[signal];
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
