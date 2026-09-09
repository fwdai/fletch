import {
  formatClock,
  type PrimaryState,
} from "@desktop/components/Composer/PrimaryControl/primaryState";
import { useElapsed } from "@desktop/components/Composer/PrimaryControl/useElapsed";
import { Icon } from "../components/Icon";

/** Bar height range, in px: silence sits at the floor, loud speech fills. */
const MIN_PX = 3;
const RANGE_PX = 19;

/** What a composer's tool row becomes while the mic is open: ✕ cancel on the
 *  far left, a full-width waveform of recent microphone levels with the elapsed
 *  clock, and — once the mic has closed — the "Transcribing" label in their
 *  place. Every exit has a button; there is no esc key here. The host row sizes
 *  it through `className` (the chat footer and the new-agent prompt give it
 *  different room). */
export function VoiceRow({
  className,
  state,
  levels,
  startedAt,
  onCancel,
}: {
  className: string;
  state: PrimaryState;
  /** Recent microphone levels, oldest first, each 0–1. */
  levels: number[];
  /** When the mic opened (epoch ms); null when it isn't open. */
  startedAt: number | null;
  onCancel: () => void;
}) {
  const listening = state === "listening";
  const transcribing = state === "transcribing";
  const elapsed = useElapsed(listening ? startedAt : null);

  return (
    <div className={className}>
      {/* Too late to cancel once the mic has closed: the button hides but keeps
       *  its space, so the label doesn't jump left. */}
      <button
        type="button"
        className="m-cancel"
        aria-label="Cancel dictation"
        tabIndex={listening ? 0 : -1}
        disabled={!listening}
        style={{ visibility: transcribing ? "hidden" : "visible" }}
        onClick={onCancel}
      >
        <span className="circ">
          <Icon name="close" size={15} />
        </span>
      </button>
      {transcribing ? (
        <div className="m-wave tx">
          Transcribing
          <span className="tdots">
            <i />
            <i />
            <i />
          </span>
        </div>
      ) : (
        <div className="m-wave">
          <span className="bars" aria-hidden="true">
            {levels.map((level, i) => (
              <i
                // Fixed slots in a scrolling window: position is the identity.
                // biome-ignore lint/suspicious/noArrayIndexKey: see above
                key={i}
                style={{ height: MIN_PX + Math.round(level * RANGE_PX) }}
              />
            ))}
          </span>
          <span className="clock">{formatClock(elapsed)}</span>
        </div>
      )}
    </div>
  );
}
