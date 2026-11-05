const R = 5.5;
const C = 2 * Math.PI * R;

/** The chip's 14px progress ring: its arc is the share of the window in use.
 *  `pct` null (the fill isn't known) draws the bare track — never a 0% arc,
 *  which would claim an empty window. Colour comes from `currentColor`. */
export function Ring({ pct }: { pct: number | null }) {
  return (
    <svg className="hu-ring" viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
      <circle cx="8" cy="8" r={R} fill="none" stroke="var(--bd-strong)" strokeWidth="2.4" />
      {pct != null && (
        <circle
          cx="8"
          cy="8"
          r={R}
          fill="none"
          stroke="currentColor"
          strokeWidth="2.4"
          strokeLinecap="round"
          strokeDasharray={C}
          strokeDashoffset={C * (1 - pct / 100)}
          transform="rotate(-90 8 8)"
        />
      )}
    </svg>
  );
}
