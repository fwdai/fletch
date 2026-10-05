import { useId } from "react";

/** Fletch brand mark: the two-blade bolt from the app icon. Filled with
 *  `currentColor` so it tracks the surrounding text color (and adapts across
 *  themes), with a soft top-to-bottom fade that echoes the icon's finish. The
 *  viewBox is the artwork's own bounds (a tall ~2:3 shape), so size it by
 *  height and let width follow, or give both and it centers. */
export function FletchMark({ className }: { className?: string }) {
  const gradientId = useId();
  return (
    <svg
      className={className}
      viewBox="12 8 341 518"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      aria-hidden="true"
      focusable="false"
    >
      <defs>
        <linearGradient
          id={gradientId}
          x1="0"
          y1="8"
          x2="0"
          y2="526"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0" stopColor="currentColor" />
          <stop offset="1" stopColor="currentColor" stopOpacity="0.78" />
        </linearGradient>
      </defs>
      <path
        fill={`url(#${gradientId})`}
        d="M182.98 34.5L12.98 269C12.98 269 118.744 269.023 124.48 269C130.216 268.977 148.09 265.305 160.332 257C169.746 250.613 180.409 236.5 180.409 236.5L341.98 8.5H231.98C231.98 8.5 219.48 9 205.98 16C192.48 23 182.98 34.5 182.98 34.5Z"
      />
      <path
        fill={`url(#${gradientId})`}
        d="M351.98 269L161.98 525.5V330.5C161.98 320.5 164.98 306 182.98 289.5C205.344 269 228.48 269 228.48 269H351.98Z"
      />
    </svg>
  );
}
