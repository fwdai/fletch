/** A shimmer placeholder for a block that hasn't loaded.
 *
 *  `height` should match what lands in its place, so the page doesn't jump
 *  when it does — which is the whole reason to prefer this over a spinner for
 *  a known-size region. Sized by the caller because only the caller knows.
 *
 *  Its inline sibling is `.stat-shimmer` (Stat.tsx), for a single value inside
 *  a line of text; both run off the same keyframes. */
export function Skeleton({ height, className }: { height: number; className?: string }) {
  return (
    <div className={className ? `blk-skeleton ${className}` : "blk-skeleton"} style={{ height }} />
  );
}
