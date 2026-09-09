/** Bar height range, in px: silence sits at the floor, loud speech fills. */
const MIN_PX = 3;
const RANGE_PX = 13;

/** The listening pill's waveform: one bar per recent microphone level (see
 *  `useDictation().levels`), oldest on the left, each 0–1. Heights animate
 *  between samples in CSS, which is the "lerp" — the samples arrive every
 *  90 ms and the transition takes as long. */
export function LevelBars({ levels }: { levels: number[] }) {
  return (
    <span className="pc-bars" aria-hidden="true">
      {levels.map((level, i) => (
        <i
          // Fixed slots in a scrolling window: position is the identity.
          // biome-ignore lint/suspicious/noArrayIndexKey: see above
          key={i}
          style={{ height: MIN_PX + Math.round(level * RANGE_PX) }}
        />
      ))}
    </span>
  );
}
